#!/usr/bin/env python3
"""Hunt's website on poopihunt.xyz: devices (phones, Macs, PCs) report their
battle and take commands; browsers log in with a device's code, then
battle from the page.

Devices make every connection (no open ports on a phone): they post their
state and screenshots, and hold GET /api/device/poll open until a command
comes. Browsers get updates over one event stream (/api/live).

Logging in: a device gets a code when its hunt starts (and a new one when
asked for). Typed on the site, it logs the browser in for 30 days and is
spent: a code works once. Each browser sees only the devices it logged in
to; a device can log every browser out of itself.

Screenshots and battle state are kept in memory only; the database holds
devices, codes (hashed) and sessions (hashed).

Logs: each device sends its hunt's log (every line, each encounter's reads, the
pictures of what went wrong) with its key, kept LOG_DAYS days. Only admins read
them, on /app/logs/: a browser logged in to one of admins.json's devices (next
to the database: ["device id", ...]).

    python hunt_server.py --port 8791 --db hunt.db
"""
import argparse
import asyncio
import base64
import collections
import hashlib
import json
import os
import re
import secrets
import sqlite3
import time

import aiohttp
from aiohttp import web

CODE_CHARS = "ABCDEFGHJKMNPQRSTUVWXYZ23456789"  # no 0/O, 1/I/L
CODE_LEN = 6
SESSION_DAYS = 30
POLL_SECONDS = 25
ONLINE_SECONDS = 40      # a device that hasn't polled for this long is offline
MAX_SHOT = 600_000       # bytes
COMMANDS = {"takeover", "resume", "ball", "move", "heal", "pp", "run", "shot"}
COOKIE = "hunt_session"
SHOWDOWN = "https://play.pokemonshowdown.com/"
ITEMS = "https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/items/"
SPRITES = {  # the battle page's pictures: the Gen 5 sprites the game itself draws
    "front": (SHOWDOWN + "sprites/gen5/{}.png",),
    "front-shiny": (SHOWDOWN + "sprites/gen5-shiny/{}.png", SHOWDOWN + "sprites/gen5/{}.png"),
    "back": (SHOWDOWN + "sprites/gen5-back/{}.png",),
    "item": (ITEMS + "{}.png",),  # bag icons: timer-ball, full-restore
}
LOG_DAYS = 7
LOG_LINES = 300_000      # per device at most (a busy PC writes ~15000 a day)
LOG_SHOTS = 400          # pictures per device at most
MAX_LOG = 400_000        # bytes in one send: 500 lines
MAX_LOG_SHOT = 900_000   # bytes: a JPEG up to 600 kB, in base64
LOG_KINDS = {"line", "problem", "detail", "note"}
WEBHOOK = re.compile(r"(?i)https?://[\w.]*discord(app)?\.com/api/webhooks/[\w/-]*")  # never kept
HISTORY = 50             # encounters kept per device (memory only), for the page's ENCOUNTERS
MAX_SPRITE = 40_000      # bytes: the 96 x 96 sprite area a device samples from the game


def sha(s):
    return hashlib.sha256(s.encode()).hexdigest()


class Limit:
    """At most n hits per key in window seconds."""

    def __init__(self, n, window):
        self.n, self.window, self.hits = n, window, {}

    def ok(self, key):
        t = time.time()
        hits = [h for h in self.hits.get(key, []) if t - h < self.window]
        if len(self.hits) > 10000:  # forget idle keys now and then
            self.hits = {k: v for k, v in self.hits.items() if v and t - v[-1] < self.window}
        if len(hits) >= self.n:
            self.hits[key] = hits
            return False
        self.hits[key] = hits + [t]
        return True


class Live:
    """What a device is doing right now (memory only)."""

    def __init__(self):
        self.state = {}
        self.state_at = 0.0
        self.shot = None
        self.shot_n = 0
        self.sprite = None       # the wild Pokemon as the game draws it: count, species, PNG
        self.history = collections.deque(maxlen=HISTORY)  # its last encounters, newest last
        self.commands = []
        self.wake = asyncio.Event()
        self.polled = 0.0
        self.seq = 0             # commands sent

    @property
    def online(self):
        return time.time() - self.polled < ONLINE_SECONDS


class Hub:
    def __init__(self, db_path):
        self.db = sqlite3.connect(db_path)
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS devices (id TEXT PRIMARY KEY, key_hash TEXT UNIQUE,
                label TEXT, kind TEXT, build TEXT, created REAL, seen REAL);
            CREATE TABLE IF NOT EXISTS codes (device TEXT PRIMARY KEY, code_hash TEXT UNIQUE, created REAL);
            CREATE TABLE IF NOT EXISTS sessions (token_hash TEXT PRIMARY KEY, created REAL, expires REAL);
            CREATE TABLE IF NOT EXISTS session_devices (token_hash TEXT, device TEXT,
                PRIMARY KEY (token_hash, device));
            CREATE TABLE IF NOT EXISTS logs (id INTEGER PRIMARY KEY, device TEXT, run TEXT, who TEXT,
                at REAL, kind TEXT, text TEXT, shot TEXT);
            CREATE INDEX IF NOT EXISTS logs_device ON logs (device, id);
            CREATE INDEX IF NOT EXISTS logs_at ON logs (at);
        """)
        self.db.commit()
        self.data = os.path.dirname(os.path.abspath(db_path))
        self.shots_dir = os.path.join(self.data, "logshots")
        self.admins_file = os.path.join(self.data, "admins.json")
        self.admins_read = (None, set())  # (mtime, device ids)
        self.log_sends = Limit(40, 60)    # per device
        self.log_shots = Limit(60, 3600)
        self.log_tidied = 0.0
        self.live = {}           # device id -> Live
        self.watchers = {}       # device id -> set of asyncio.Queue (browsers' event streams)
        self.new_devices = Limit(20, 3600)
        self.codes_tried = Limit(10, 600)

    def dev(self, device):
        return self.live.setdefault(device, Live())

    # ------------------------------------------------------------ devices

    def device_for(self, request, create=False):
        """The device a request's key belongs to; with create, a new key is a new device."""
        auth = request.headers.get("Authorization", "")
        if not auth.startswith("Bearer ") or len(auth) < 40:
            raise web.HTTPUnauthorized(text="no device key")
        h = sha(auth[7:].strip())
        row = self.db.execute("SELECT id FROM devices WHERE key_hash = ?", (h,)).fetchone()
        if row:
            self.db.execute("UPDATE devices SET seen = ? WHERE id = ?", (time.time(), row[0]))
            return row[0]
        if not create:
            raise web.HTTPUnauthorized(text="unknown device")
        if not self.new_devices.ok(ip(request)):
            raise web.HTTPTooManyRequests(text="too many new devices")
        device = secrets.token_urlsafe(9)
        self.db.execute("INSERT INTO devices VALUES (?, ?, '', '', '', ?, ?)", (device, h, time.time(), time.time()))
        self.db.commit()
        return device

    def new_code(self, device):
        while True:
            code = "".join(secrets.choice(CODE_CHARS) for _ in range(CODE_LEN))
            try:
                self.db.execute("INSERT OR REPLACE INTO codes VALUES (?, ?, ?)", (device, sha(code), time.time()))
                self.db.commit()
                return code
            except sqlite3.IntegrityError:  # another device has it
                continue

    def spend_code(self, device):
        self.db.execute("DELETE FROM codes WHERE device = ?", (device,))
        self.db.commit()

    def command(self, device, cmd):
        d = self.dev(device)
        d.seq += 1
        d.commands.append(dict(cmd, seq=d.seq))
        d.wake.set()
        return d.seq

    def label(self, device):
        row = self.db.execute("SELECT label, kind, build FROM devices WHERE id = ?", (device,)).fetchone()
        return {"label": row[0], "kind": row[1], "build": row[2]} if row else {}

    # ------------------------------------------------------------ browsers

    def session_devices(self, request):
        """The devices this browser logged in to; 401 if it isn't logged in."""
        token = request.cookies.get(COOKIE, "")
        if not token:
            raise web.HTTPUnauthorized(text="not logged in")
        h = sha(token)
        row = self.db.execute("SELECT expires FROM sessions WHERE token_hash = ?", (h,)).fetchone()
        if not row or row[0] < time.time():
            raise web.HTTPUnauthorized(text="not logged in")
        return h, [r[0] for r in self.db.execute(
            "SELECT device FROM session_devices WHERE token_hash = ?", (h,))]

    def owned(self, request, device):
        _, devices = self.session_devices(request)
        if device not in devices:
            raise web.HTTPNotFound(text="no such device")  # someone else's looks the same as none
        return device

    def view(self, device):
        """A device as the page sees it."""
        d = self.dev(device)
        return dict(self.label(device), id=device, online=d.online, state=d.state,
                    state_age=round(time.time() - d.state_at, 1) if d.state_at else None, shot=d.shot_n,
                    sprite={"count": d.sprite["count"], "species": d.sprite["species"]} if d.sprite else None)

    def push(self, device):
        msg = json.dumps(self.view(device))
        for q in self.watchers.get(device, ()):
            if q.qsize() < 50:
                q.put_nowait(msg)

    # ------------------------------------------------------------ logs

    def admins(self):
        try:
            mtime = os.path.getmtime(self.admins_file)
        except OSError:
            return set()
        if self.admins_read[0] != mtime:
            try:
                with open(self.admins_file) as f:
                    ids = json.load(f)
                self.admins_read = (mtime, {str(i) for i in ids} if isinstance(ids, list) else set())
            except (OSError, ValueError):
                self.admins_read = (mtime, set())
        return self.admins_read[1]

    def admin(self, request):
        """A browser logged in to an admin's device; 401 / 403 otherwise."""
        _, devices = self.session_devices(request)
        if not self.admins() & set(devices):
            raise web.HTTPForbidden(text="not an admin")

    def shot_path(self, device, name):
        return os.path.join(self.shots_dir, device, name)

    def drop_logs(self, where, args):
        for device, shot in self.db.execute(f"SELECT device, shot FROM logs WHERE shot != '' AND {where}", args).fetchall():
            try:
                os.remove(self.shot_path(device, shot))
            except OSError:
                pass
        self.db.execute(f"DELETE FROM logs WHERE {where}", args)

    def tidy_logs(self):
        """Older than LOG_DAYS, and past each device's share, go."""
        self.drop_logs("at < ?", (time.time() - LOG_DAYS * 86400,))
        for device, n in self.db.execute("SELECT device, COUNT(*) FROM logs GROUP BY device").fetchall():
            if n > LOG_LINES:
                cut = self.db.execute("SELECT id FROM logs WHERE device = ? ORDER BY id DESC LIMIT 1 OFFSET ?",
                                      (device, LOG_LINES)).fetchone()[0]
                self.drop_logs("device = ? AND id <= ?", (device, cut))
            shots = self.db.execute("SELECT COUNT(*) FROM logs WHERE device = ? AND shot != ''", (device,)).fetchone()[0]
            if shots > LOG_SHOTS:
                cut = self.db.execute("SELECT id FROM logs WHERE device = ? AND shot != '' ORDER BY id DESC LIMIT 1 OFFSET ?",
                                      (device, LOG_SHOTS)).fetchone()[0]
                self.drop_logs("device = ? AND shot != '' AND id <= ?", (device, cut))
        self.db.commit()

    def tidy(self):
        t = time.time()
        self.db.execute("DELETE FROM sessions WHERE expires < ?", (t,))
        self.db.execute("DELETE FROM session_devices WHERE token_hash NOT IN (SELECT token_hash FROM sessions)")
        self.db.commit()
        if t - self.log_tidied > 600:
            self.log_tidied = t
            self.tidy_logs()


class Showdown:
    """Pokemon pictures, bag icons and move types for the battle page (from Pokemon
    Showdown and PokeAPI); fetched once each and kept in memory, so the page never
    talks to another site."""

    def __init__(self):
        self.http = None         # an aiohttp.ClientSession while the app runs
        self.sprites = {}        # (kind, id) -> (bytes, content type), or None if there's none
        self.moves = None        # move id -> {"type", "category"}
        self.moves_tried = 0.0
        self.lock = asyncio.Lock()

    async def get(self, url):
        async with self.http.get(url, timeout=aiohttp.ClientTimeout(total=10)) as r:
            if r.status != 200 or not r.content_type.startswith(("image/", "application/json")):
                return None, None
            return await r.read(), r.content_type

    async def sprite(self, kind, name):
        key = (kind, name)
        if key not in self.sprites:
            got = None
            for url in SPRITES[kind]:
                try:
                    data, ctype = await self.get(url.format(name))
                except (aiohttp.ClientError, asyncio.TimeoutError):
                    return None  # not remembered: tried again next time
                if data:
                    got = data, ctype
                    break
            if len(self.sprites) > 600:
                self.sprites.clear()
            self.sprites[key] = got
        return self.sprites[key]

    async def move(self, name):
        async with self.lock:
            if self.moves is None and time.time() - self.moves_tried > 600:
                self.moves_tried = time.time()
                try:
                    data, _ = await self.get(SHOWDOWN + "data/moves.json")
                    self.moves = {k: {"type": v.get("type", ""), "category": v.get("category", "")}
                                  for k, v in json.loads(data).items()} if data else None
                except (aiohttp.ClientError, asyncio.TimeoutError, ValueError):
                    pass
        return (self.moves or {}).get(name)


def ip(request):
    return request.headers.get("X-Real-IP") or request.remote or "?"


async def body(request, limit=20000):
    if request.content_length and request.content_length > limit:
        raise web.HTTPRequestEntityTooLarge(max_size=limit, actual_size=request.content_length)
    try:
        data = await request.json()
    except Exception:
        raise web.HTTPBadRequest(text="not JSON")
    if not isinstance(data, dict):
        raise web.HTTPBadRequest(text="not an object")
    return data


def same_origin(request):
    """Browser writes only from the site itself (with the SameSite cookie, against CSRF)."""
    origin = request.headers.get("Origin")
    if origin and origin.split("://", 1)[-1] != request.headers.get("Host", ""):
        raise web.HTTPForbidden(text="wrong origin")


# ---------------------------------------------------------------- device API

async def device_start(request):
    """A hunt started: a new code (the old one is gone)."""
    hub = request.app["hub"]
    device = hub.device_for(request, create=True)
    data = await body(request)
    hub.db.execute("UPDATE devices SET label = ?, kind = ?, build = ? WHERE id = ?",
                   (str(data.get("label", ""))[:80], str(data.get("kind", ""))[:20],
                    str(data.get("build", ""))[:40], device))
    code = hub.new_code(device)
    d = hub.dev(device)
    d.polled = time.time()
    hub.push(device)
    return web.json_response({"code": code})


async def device_stop(request):
    hub = request.app["hub"]
    device = hub.device_for(request)
    hub.spend_code(device)
    d = hub.dev(device)
    d.state = {"hunting": False}
    d.state_at = time.time()
    d.polled = 0
    d.commands.clear()
    hub.push(device)
    return web.json_response({})


async def device_state(request):
    hub = request.app["hub"]
    device = hub.device_for(request)
    data = await body(request)
    d = hub.dev(device)
    d.state = data
    d.state_at = time.time()
    d.polled = time.time()
    # each new encounter, for the page's list of them
    enc, count = data.get("enc"), data.get("count")
    if data.get("phase") == "battle" and isinstance(enc, dict) and enc.get("species") and \
            (not d.history or d.history[-1]["count"] != count):
        d.history.append({"count": count, "at": round(time.time()), "wanted": bool(data.get("wanted")),
                          **{k: enc.get(k) for k in ("species", "level", "shiny", "tint", "reskin", "gradient")}})
    hub.push(device)
    return web.json_response({})


async def device_sprite(request):
    """The wild Pokemon's 96 x 96 sprite area, sampled from the game by the device."""
    hub = request.app["hub"]
    device = hub.device_for(request)
    data = await body(request, MAX_SPRITE * 2)
    try:
        png = base64.b64decode(str(data.get("png", "")), validate=True)
    except ValueError:
        raise web.HTTPBadRequest(text="not base64")
    if len(png) > MAX_SPRITE or not png.startswith(b"\x89PNG\r\n\x1a\n"):
        raise web.HTTPBadRequest(text="a PNG up to 40 kB")
    count = data.get("count")
    d = hub.dev(device)
    d.sprite = {"count": count if isinstance(count, int) else None,
                "species": str(data.get("species", ""))[:40], "png": png}
    hub.push(device)
    return web.json_response({})


async def device_shot(request):
    hub = request.app["hub"]
    device = hub.device_for(request)
    jpg = await request.content.read(MAX_SHOT + 1)
    if len(jpg) > MAX_SHOT or not jpg.startswith(b"\xff\xd8"):
        raise web.HTTPBadRequest(text="a JPEG up to 600 kB")
    d = hub.dev(device)
    d.shot = jpg
    d.shot_n += 1
    hub.push(device)
    return web.json_response({})


async def device_poll(request):
    """Held open until a command comes (or POLL_SECONDS)."""
    hub = request.app["hub"]
    device = hub.device_for(request)
    d = hub.dev(device)
    was_online = d.online
    d.polled = time.time()
    if not was_online:
        hub.push(device)
    if not d.commands:
        d.wake.clear()
        try:
            await asyncio.wait_for(d.wake.wait(), POLL_SECONDS)
        except asyncio.TimeoutError:
            pass
    d.polled = time.time()
    commands, d.commands = d.commands, []
    return web.json_response({"commands": commands})


async def device_code(request):
    """A new code (asked for on the device); the old one stops working."""
    hub = request.app["hub"]
    device = hub.device_for(request)
    return web.json_response({"code": hub.new_code(device)})


async def device_logout(request):
    """Logs every browser out of this device (asked for on the device)."""
    hub = request.app["hub"]
    device = hub.device_for(request)
    hub.db.execute("DELETE FROM session_devices WHERE device = ?", (device,))
    hub.db.commit()
    return web.json_response({})


def log_text(s, n):
    return WEBHOOK.sub("[webhook]", str(s))[:n]


def log_at(v):
    """A device's time (ms) for a line, or now if it's far off (a wrong clock)."""
    t = v / 1000 if isinstance(v, (int, float)) else 0
    return t if abs(t - time.time()) < 3 * 86400 else time.time()


async def device_log(request):
    """Lines of the hunt's log: {"run", "who", "lines": [[ms, kind, text], ...]}."""
    hub = request.app["hub"]
    device = hub.device_for(request)
    if not hub.log_sends.ok(device):
        raise web.HTTPTooManyRequests(text="slow down")
    data = await body(request, MAX_LOG)
    lines = data.get("lines")
    if not isinstance(lines, list) or len(lines) > 500:
        raise web.HTTPBadRequest(text="up to 500 lines")
    run, who = log_text(data.get("run", ""), 16), log_text(data.get("who", ""), 120)
    rows = []
    for line in lines:
        if not (isinstance(line, list) and len(line) == 3 and line[1] in LOG_KINDS):
            raise web.HTTPBadRequest(text="a line is [ms, kind, text]")
        rows.append((device, run, who, log_at(line[0]), line[1], log_text(line[2], 2000), ""))
    # a batch sent again (its answer lost on the way back, so the Hunt kept it to resend):
    # the lines already here aren't added twice
    if rows:
        ats = [r[3] for r in rows]
        have = set(hub.db.execute("SELECT at, kind, text FROM logs WHERE device = ? AND run = ? AND at BETWEEN ? AND ?",
                                  (device, run, min(ats), max(ats))).fetchall())
        rows = [r for r in rows if (r[3], r[4], r[5]) not in have]
    hub.db.executemany("INSERT INTO logs (device, run, who, at, kind, text, shot) VALUES (?, ?, ?, ?, ?, ?, ?)", rows)
    hub.db.commit()
    return web.json_response({})


async def device_log_shot(request):
    """A picture for the log: {"run", "who", "at", "caption", "jpg": base64}."""
    hub = request.app["hub"]
    device = hub.device_for(request)
    if not hub.log_shots.ok(device):
        raise web.HTTPTooManyRequests(text="slow down")
    data = await body(request, MAX_LOG_SHOT)
    try:
        jpg = base64.b64decode(str(data.get("jpg", "")), validate=True)
    except ValueError:
        raise web.HTTPBadRequest(text="not base64")
    if len(jpg) > MAX_SHOT or not jpg.startswith(b"\xff\xd8"):
        raise web.HTTPBadRequest(text="a JPEG up to 600 kB")
    name = secrets.token_hex(8) + ".jpg"
    os.makedirs(os.path.join(hub.shots_dir, device), exist_ok=True)
    with open(hub.shot_path(device, name), "wb") as f:
        f.write(jpg)
    hub.db.execute("INSERT INTO logs (device, run, who, at, kind, text, shot) VALUES (?, ?, ?, ?, 'shot', ?, ?)",
                   (device, log_text(data.get("run", ""), 16), log_text(data.get("who", ""), 120), log_at(data.get("at")),
                    log_text(data.get("caption", ""), 300), name))
    hub.db.commit()
    return web.json_response({})


# ---------------------------------------------------------------- logging in

async def login_code(request):
    """The code alone logs in, once."""
    hub = request.app["hub"]
    same_origin(request)
    if not hub.codes_tried.ok(ip(request)):
        raise web.HTTPTooManyRequests(text="Too many tries. Wait 10 minutes.")
    code = re.sub(r"[^A-Z0-9]", "", str((await body(request)).get("code", "")).upper())
    row = hub.db.execute("SELECT device FROM codes WHERE code_hash = ?", (sha(code),)).fetchone() \
        if len(code) == CODE_LEN else None
    if not row:
        raise web.HTTPNotFound(text="That code isn't right. Hunt shows a new one each time it starts.")
    device = row[0]
    hub.spend_code(device)
    token = request.cookies.get(COOKIE, "")
    h = sha(token) if token else ""
    row = hub.db.execute("SELECT expires FROM sessions WHERE token_hash = ?", (h,)).fetchone() if h else None
    if not row or row[0] < time.time():  # a new session; else this device joins the browser's
        token = secrets.token_urlsafe(32)
        h = sha(token)
        hub.db.execute("INSERT INTO sessions VALUES (?, ?, ?)", (h, time.time(), time.time() + SESSION_DAYS * 86400))
    hub.db.execute("INSERT OR IGNORE INTO session_devices VALUES (?, ?)", (h, device))
    hub.db.commit()
    hub.command(device, {"cmd": "login", "result": "ok", "browser": request.headers.get("User-Agent", "")[:120]})
    resp = web.json_response({"ok": True, "device": device})
    resp.set_cookie(COOKIE, token, max_age=SESSION_DAYS * 86400, path="/api", secure=request.app["secure"],
                    httponly=True, samesite="Strict")
    return resp


async def logout(request):
    hub = request.app["hub"]
    same_origin(request)
    token = request.cookies.get(COOKIE, "")
    if token:
        hub.db.execute("DELETE FROM sessions WHERE token_hash = ?", (sha(token),))
        hub.db.execute("DELETE FROM session_devices WHERE token_hash = ?", (sha(token),))
        hub.db.commit()
    resp = web.json_response({})
    resp.del_cookie(COOKIE, path="/api")
    return resp


async def forget(request):
    """Takes a device off this browser."""
    hub = request.app["hub"]
    same_origin(request)
    h, _ = hub.session_devices(request)
    device = hub.owned(request, request.match_info["device"])
    hub.db.execute("DELETE FROM session_devices WHERE token_hash = ? AND device = ?", (h, device))
    hub.db.commit()
    return web.json_response({})


# ---------------------------------------------------------------- the page's API

async def me(request):
    hub = request.app["hub"]
    h, devices = hub.session_devices(request)
    resp = web.json_response({"devices": [hub.view(d) for d in devices]})
    # a login in use doesn't run out: back to the full 30 days, once a day at most
    expires = hub.db.execute("SELECT expires FROM sessions WHERE token_hash = ?", (h,)).fetchone()[0]
    if expires - time.time() < (SESSION_DAYS - 1) * 86400:
        hub.db.execute("UPDATE sessions SET expires = ? WHERE token_hash = ?", (time.time() + SESSION_DAYS * 86400, h))
        hub.db.commit()
        resp.set_cookie(COOKIE, request.cookies[COOKIE], max_age=SESSION_DAYS * 86400, path="/api",
                        secure=request.app["secure"], httponly=True, samesite="Strict")
    return resp


async def history(request):
    """A device's last encounters, newest first."""
    hub = request.app["hub"]
    device = hub.owned(request, request.match_info["device"])
    return web.json_response({"encounters": list(reversed(hub.dev(device).history))})


async def live(request):
    """Server-sent events: every change to this browser's devices."""
    hub = request.app["hub"]
    _, devices = hub.session_devices(request)
    resp = web.StreamResponse(headers={"Content-Type": "text/event-stream", "Cache-Control": "no-cache",
                                       "X-Accel-Buffering": "no"})
    await resp.prepare(request)
    q = asyncio.Queue()
    for d in devices:
        hub.watchers.setdefault(d, set()).add(q)
        q.put_nowait(json.dumps(hub.view(d)))
    try:
        while True:
            try:
                msg = await asyncio.wait_for(q.get(), 15)
                await resp.write(f"data: {msg}\n\n".encode())
            except asyncio.TimeoutError:
                for d in devices:  # online / offline changes, and a keepalive
                    q.put_nowait(json.dumps(hub.view(d)))
    except (ConnectionResetError, asyncio.CancelledError):
        pass
    finally:
        for d in devices:
            hub.watchers.get(d, set()).discard(q)
    return resp


async def shot(request):
    hub = request.app["hub"]
    device = hub.owned(request, request.match_info["device"])
    jpg = hub.dev(device).shot
    if not jpg:
        raise web.HTTPNotFound(text="no picture yet")
    return web.Response(body=jpg, content_type="image/jpeg", headers={"Cache-Control": "private, no-store"})


async def game_sprite(request):
    hub = request.app["hub"]
    device = hub.owned(request, request.match_info["device"])
    sprite = hub.dev(device).sprite
    if not sprite:
        raise web.HTTPNotFound(text="no sprite yet")
    return web.Response(body=sprite["png"], content_type="image/png", headers={"Cache-Control": "private, no-store"})


async def send_command(request):
    hub = request.app["hub"]
    same_origin(request)
    device = hub.owned(request, request.match_info["device"])
    data = await body(request)
    d = hub.dev(device)
    cmd = data.get("cmd")
    if cmd not in COMMANDS:
        raise web.HTTPBadRequest(text="unknown command")
    if not d.online:
        raise web.HTTPConflict(text="That device is offline.")
    out = {"cmd": cmd}
    if cmd in ("ball", "move"):
        offered = d.state.get("balls", []) if cmd == "ball" else [m.get("name") for m in d.state.get("moves", [])]
        if data.get("name") not in offered:
            raise web.HTTPBadRequest(text=f"no such {cmd}")
        out["name"] = data["name"]
    return web.json_response({"seq": hub.command(device, out)})


async def sprite(request):
    """A Pokemon's picture, e.g. /api/sprite/front-shiny/dewpider (logged-in browsers only)."""
    request.app["hub"].session_devices(request)
    kind, name = request.match_info["kind"], request.match_info["name"]
    if kind not in SPRITES or not re.fullmatch(r"[a-z0-9-]{1,40}", name):
        raise web.HTTPNotFound(text="no such picture")
    got = await request.app["showdown"].sprite(kind, name)
    if not got:
        raise web.HTTPNotFound(text="no such picture")
    return web.Response(body=got[0], content_type=got[1], headers={"Cache-Control": "private, max-age=604800"})


async def move_info(request):
    """A move's type and category, e.g. /api/move/yawn -> {"type": "Normal", "category": "Status"}."""
    request.app["hub"].session_devices(request)
    name = request.match_info["name"]
    info = re.fullmatch(r"[a-z0-9]{1,40}", name) and await request.app["showdown"].move(name)
    if not info:
        raise web.HTTPNotFound(text="no such move")
    return web.json_response(info, headers={"Cache-Control": "private, max-age=604800"})


# ---------------------------------------------------------------- the logs page (admins)

async def logs_devices(request):
    """Every device, newest seen first, with how much it's logged and its last problem."""
    hub = request.app["hub"]
    hub.admin(request)
    stats = {d: (n, p) for d, n, p in hub.db.execute(
        "SELECT device, COUNT(*), MAX(CASE WHEN kind = 'problem' OR kind = 'shot' THEN at END) FROM logs GROUP BY device")}
    out = []
    for device, label, kind, build, seen in hub.db.execute(
            "SELECT id, label, kind, build, seen FROM devices ORDER BY seen DESC"):
        n, problem = stats.get(device, (0, None))
        out.append({"id": device, "label": label, "kind": kind, "build": build, "seen": seen,
                    "online": hub.dev(device).online if device in hub.live else False,
                    "lines": n, "problem": problem})
    return web.json_response({"devices": out})


async def logs_lines(request):
    """A device's log, newest first: ?before=id (older than), q= (text in it), problems=1
    (what went wrong, pictures, notes), limit= (500 at most)."""
    hub = request.app["hub"]
    hub.admin(request)
    device = request.match_info["device"]
    q = request.query
    where, args = ["device = ?"], [device]
    if q.get("before", "").isdigit():
        where.append("id < ?")
        args.append(int(q["before"]))
    if q.get("q"):
        where.append("text LIKE ? ESCAPE '\\'")
        args.append("%" + re.sub(r"([%_\\])", r"\\\1", q["q"][:100]) + "%")
    if q.get("problems") == "1":
        where.append("kind IN ('problem', 'shot', 'note')")
    limit = min(int(q["limit"]) if q.get("limit", "").isdigit() else 300, 500)
    rows = hub.db.execute(f"SELECT id, run, who, at, kind, text, shot FROM logs WHERE {' AND '.join(where)} "
                          "ORDER BY id DESC LIMIT ?", (*args, limit)).fetchall()
    return web.json_response({"lines": [{"id": i, "run": r, "who": w, "at": a, "kind": k, "text": t, "shot": s}
                                        for i, r, w, a, k, t, s in rows]})


async def logs_shot(request):
    hub = request.app["hub"]
    hub.admin(request)
    device, name = request.match_info["device"], request.match_info["name"]
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,40}", device) or not re.fullmatch(r"[0-9a-f]{16}\.jpg", name):
        raise web.HTTPNotFound(text="no such picture")
    try:
        with open(hub.shot_path(device, name), "rb") as f:
            jpg = f.read()
    except OSError:
        raise web.HTTPNotFound(text="no such picture")
    return web.Response(body=jpg, content_type="image/jpeg", headers={"Cache-Control": "private, max-age=604800"})


# ---------------------------------------------------------------- app

@web.middleware
async def no_store(request, handler):
    resp = await handler(request)
    resp.headers.setdefault("Cache-Control", "no-store")
    return resp


def make_app(db_path, secure=True):
    app = web.Application(middlewares=[no_store], client_max_size=MAX_LOG_SHOT + 1024)
    app["hub"] = Hub(db_path)
    app["secure"] = secure
    app["showdown"] = Showdown()
    app.add_routes([
        web.post("/api/device/start", device_start),
        web.post("/api/device/stop", device_stop),
        web.post("/api/device/state", device_state),
        web.post("/api/device/shot", device_shot),
        web.post("/api/device/sprite", device_sprite),
        web.get("/api/device/poll", device_poll),
        web.post("/api/device/code", device_code),
        web.post("/api/device/logout", device_logout),
        web.post("/api/device/log", device_log),
        web.post("/api/device/log/shot", device_log_shot),
        web.get("/api/logs/devices", logs_devices),
        web.get("/api/logs/{device}", logs_lines),
        web.get("/api/logs/{device}/shot/{name}", logs_shot),
        web.post("/api/login", login_code),
        web.post("/api/logout", logout),
        web.get("/api/me", me),
        web.get("/api/live", live),
        web.get("/api/devices/{device}/shot", shot),
        web.get("/api/devices/{device}/sprite", game_sprite),
        web.get("/api/devices/{device}/history", history),
        web.post("/api/devices/{device}/command", send_command),
        web.post("/api/devices/{device}/forget", forget),
        web.get("/api/sprite/{kind}/{name}", sprite),
        web.get("/api/move/{name}", move_info),
    ])

    async def showdown_http(app):
        async with aiohttp.ClientSession(headers={"User-Agent": "Hunt (poopihunt.xyz)"}) as http:
            app["showdown"].http = http
            yield

    app.cleanup_ctx.append(showdown_http)

    async def tidy_loop(app):
        async def loop():
            while True:
                await asyncio.sleep(5)
                app["hub"].tidy()
        task = asyncio.create_task(loop())
        yield
        task.cancel()

    app.cleanup_ctx.append(tidy_loop)
    return app


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--port", type=int, default=8791)
    p.add_argument("--db", default="hunt.db")
    p.add_argument("--static", help="serve the page from this folder too (testing without nginx)")
    p.add_argument("--insecure", action="store_true", help="cookies over plain http (testing)")
    a = p.parse_args()
    app = make_app(a.db, secure=not a.insecure)
    if a.static:
        app.router.add_get("/", lambda r: web.HTTPFound("/app/"))
        app.router.add_get("/app/", lambda r: web.FileResponse(f"{a.static}/app/index.html"))
        app.router.add_get("/app/logs/", lambda r: web.FileResponse(f"{a.static}/app/logs/index.html"))
        app.router.add_static("/", a.static)
    web.run_app(app, host=a.host, port=a.port, access_log=None)
