#!/usr/bin/env python3
"""Checks the website's rules with pretend devices and browsers:

    python server/test_server.py
"""
import asyncio
import json
import os
import secrets
import shutil
import sys
import tempfile

from aiohttp.test_utils import TestClient, TestServer

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import hunt_server  # noqa: E402

async def browser(server):
    c = TestClient(server)
    await c.start_server()
    return c


async def main():
    folder = tempfile.mkdtemp()
    db = os.path.join(folder, "hunt.db")
    app = hunt_server.make_app(db, secure=False)
    server = TestServer(app)
    await server.start_server()

    async def new_device():
        c = TestClient(server)
        await c.start_server()
        c.session.headers["Authorization"] = "Bearer " + secrets.token_urlsafe(32)
        r = await c.post("/api/device/start", json={"label": "Phone", "kind": "test", "build": "1"})
        assert r.status == 200
        return c, (await r.json())["code"]

    mine, code = await new_device()
    theirs, their_code = await new_device()
    assert code != their_code and len(code) == 6

    me = await browser(server)
    assert (await me.get("/api/me")).status == 401                            # not logged in
    assert (await me.post("/api/login", json={"code": "ZZZZZZ"})).status == 404  # no such code

    # a new code (asked for on the device) replaces the old one
    old, code = code, (await (await mine.post("/api/device/code")).json())["code"]
    assert (await me.post("/api/login", json={"code": old})).status == 404

    # the code logs in (any case, any spacing), and works once
    r = await me.post("/api/login", json={"code": f" {code[:3].lower()} {code[3:]}"})
    assert r.status == 200, await r.text()
    device_id = (await r.json())["device"]
    assert (await me.post("/api/login", json={"code": code})).status == 404
    assert [d["id"] for d in (await (await me.get("/api/me")).json())["devices"]] == [device_id]
    poll = await (await mine.get("/api/device/poll")).json()
    assert poll["commands"][0]["result"] == "ok"  # the device is told

    # the other person logs in to theirs and sees only theirs
    them = await browser(server)
    r = await them.post("/api/login", json={"code": their_code})
    assert r.status == 200
    their_id = (await r.json())["device"]
    assert [d["id"] for d in (await (await them.get("/api/me")).json())["devices"]] == [their_id]
    # ... and can't touch mine
    online = asyncio.create_task(mine.get("/api/device/poll"))  # mine is online, waiting for commands
    await asyncio.sleep(0.05)
    assert (await them.post(f"/api/devices/{device_id}/command", json={"cmd": "takeover"})).status == 404
    assert (await them.get(f"/api/devices/{device_id}/shot")).status == 404
    assert (await them.post(f"/api/devices/{device_id}/forget", json={})).status == 404
    # nor can a browser without a session, or a device key on the page's API
    anon = await browser(server)
    assert (await anon.post(f"/api/devices/{device_id}/command", json={"cmd": "takeover"})).status == 401
    assert (await mine.get("/api/me")).status == 401
    # pictures and moves are for logged-in browsers, and only names that look like names
    assert (await anon.get("/api/sprite/front/dewpider")).status == 401
    assert (await anon.get("/api/move/yawn")).status == 401
    assert (await me.get("/api/sprite/secret/dewpider")).status == 404
    assert (await me.get("/api/sprite/front/..%2Fetc")).status == 404
    assert (await me.get("/api/move/Yawn!")).status == 404

    # commands: only known ones, balls and moves the device offers, same origin only
    await mine.post("/api/device/state", json={"hunting": True, "balls": ["Timer Ball"],
                                               "moves": [{"name": "Yawn", "pp": 10, "max": 10}]})
    url = f"/api/devices/{device_id}/command"
    assert (await me.post(url, json={"cmd": "takeover"})).status == 200
    assert (await me.post(url, json={"cmd": "ball", "name": "Master Ball"})).status == 400
    assert (await me.post(url, json={"cmd": "move", "name": "Yawn"})).status == 200
    assert (await me.post(url, json={"cmd": "tap", "x": 1})).status == 400
    assert (await me.post(url, json={"cmd": "run"}, headers={"Origin": "https://evil.example"})).status == 403
    got = (await (await online).json())["commands"] + (await (await mine.get("/api/device/poll")).json())["commands"]
    assert [c["cmd"] for c in got] == ["takeover", "move"], got

    # a picture reaches its owner only
    jpg = b"\xff\xd8" + b"0" * 100
    assert (await mine.post("/api/device/shot", data=jpg, headers={"Content-Type": "image/jpeg"})).status == 200
    assert await (await me.get(f"/api/devices/{device_id}/shot")).read() == jpg
    assert (await mine.post("/api/device/shot", data=b"<html>")).status == 400

    # the wild Pokemon's sprite: PNGs only, to its owner only
    import base64
    png = b"\x89PNG\r\n\x1a\n" + b"0" * 100
    assert (await mine.post("/api/device/sprite", json={"count": 3, "species": "Dewpider",
                                                         "png": base64.b64encode(png).decode()})).status == 200
    assert await (await me.get(f"/api/devices/{device_id}/sprite")).read() == png
    assert (await them.get(f"/api/devices/{device_id}/sprite")).status == 404
    assert (await mine.post("/api/device/sprite", json={"png": base64.b64encode(b"<svg>").decode()})).status == 400
    assert (await mine.post("/api/device/sprite", json={"png": "not base64!"})).status == 400
    view = (await (await me.get("/api/me")).json())["devices"][0]
    assert view["sprite"] == {"count": 3, "species": "Dewpider"}, view

    # each new encounter is kept for the page's list, to its owner only
    for n, species in ((4, "Sentret"), (4, "Sentret"), (5, "Marill")):
        await mine.post("/api/device/state", json={"hunting": True, "phase": "battle", "count": n,
                                                    "enc": {"species": species, "level": "3", "shiny": n == 5}})
    hist = (await (await me.get(f"/api/devices/{device_id}/history")).json())["encounters"]
    assert [(e["count"], e["species"], e["shiny"]) for e in hist] == [(5, "Marill", True), (4, "Sentret", False)], hist
    assert (await them.get(f"/api/devices/{device_id}/history")).status == 404

    # the hunt's log: lines and pictures with the device's key; webhooks never kept
    hook = "https://discord.com/api/webhooks/123456/abcDEF-gh"
    lines = [[1000 * hunt_server.time.time(), "line", f"#{n} Zubat Lv50  run"] for n in range(3)]
    lines += [[0, "detail", "    reads: Zubat Lv50 (reskin DOODLE) / Zubat Lv50 (reskin DOODLE)"],
              [None, "problem", f"couldn't post to Discord: {hook}: status 404"]]
    assert (await mine.post("/api/device/log", json={"run": "ab12", "who": "Windows 11 · 1", "lines": lines})).status == 200
    assert (await mine.post("/api/device/log", json={"lines": [[0, "secret", "x"]]})).status == 400
    assert (await mine.post("/api/device/log", json={"lines": [[0, "line", "x"]] * 501})).status == 400
    assert (await anon.post("/api/device/log", json={"lines": []})).status == 401
    shot_jpg = b"\xff\xd8" + b"1" * 100
    assert (await mine.post("/api/device/log/shot", json={"run": "ab12", "caption": "stuck", "at": 0,
                                                           "jpg": base64.b64encode(shot_jpg).decode()})).status == 200
    assert (await mine.post("/api/device/log/shot", json={"jpg": base64.b64encode(b"<html>").decode()})).status == 400
    # read by admins only: a browser logged in to a device in admins.json
    assert (await anon.get("/api/logs/devices")).status == 401
    assert (await me.get("/api/logs/devices")).status == 403
    with open(os.path.join(folder, "admins.json"), "w") as f:
        json.dump([device_id], f)
    assert (await them.get("/api/logs/devices")).status == 403
    assert (await them.get(f"/api/logs/{device_id}")).status == 403
    devs = (await (await me.get("/api/logs/devices")).json())["devices"]
    assert [(d["id"], d["lines"]) for d in devs if d["lines"]] == [(device_id, 6)], devs
    got = (await (await me.get(f"/api/logs/{device_id}")).json())["lines"]
    assert [g["kind"] for g in got] == ["shot", "problem", "detail", "line", "line", "line"], got
    assert hook not in json.dumps(got) and "[webhook]: status 404" in got[1]["text"], got[1]
    assert got[0]["who"] == "" and got[1]["who"] == "Windows 11 · 1"
    assert [g["text"] for g in (await (await me.get(f"/api/logs/{device_id}?q=%231 Z")).json())["lines"]] == ["#1 Zubat Lv50  run"]
    assert (await (await me.get(f"/api/logs/{device_id}?q=100%25")).json())["lines"] == []  # % is a %
    assert [g["kind"] for g in (await (await me.get(f"/api/logs/{device_id}?problems=1")).json())["lines"]] == ["shot", "problem"]
    older = (await (await me.get(f"/api/logs/{device_id}?before={got[2]['id']}&limit=2")).json())["lines"]
    assert [g["id"] for g in older] == [got[3]["id"], got[4]["id"]], older
    url = f"/api/logs/{device_id}/shot/{got[0]['shot']}"
    assert await (await me.get(url)).read() == shot_jpg
    assert (await them.get(url)).status == 403
    assert (await me.get(f"/api/logs/{device_id}/shot/..%2F..%2Fhunt.db")).status == 404
    # a week on, gone (the picture too)
    hub = app["hub"]
    hub.db.execute("UPDATE logs SET at = at - 8 * 86400 WHERE kind != 'line'")
    hub.tidy_logs()
    assert [g["kind"] for g in (await (await me.get(f"/api/logs/{device_id}")).json())["lines"]] == ["line"] * 3
    assert (await me.get(url)).status == 404

    # a login in use is renewed: the session runs a full 30 days again from now
    token_hash = me.session.cookie_jar.filter_cookies(server.make_url("/api/me"))["hunt_session"].value
    h = hunt_server.sha(token_hash)
    app["hub"].db.execute("UPDATE sessions SET expires = ? WHERE token_hash = ?", (hunt_server.time.time() + 86400, h))
    assert (await me.get("/api/me")).status == 200
    left = app["hub"].db.execute("SELECT expires FROM sessions WHERE token_hash = ?", (h,)).fetchone()[0] - hunt_server.time.time()
    assert left > (hunt_server.SESSION_DAYS - 1) * 86400, left

    # wrong codes are limited
    hits = [(await anon.post("/api/login", json={"code": "AAAAAA"}, headers={"X-Real-IP": "10.0.0.9"})).status
            for _ in range(12)]
    assert hits[-1] == 429, hits

    # stopping spends the code
    code = (await (await theirs.post("/api/device/start", json={})).json())["code"]
    await theirs.post("/api/device/stop")
    assert (await me.post("/api/login", json={"code": code})).status == 404

    # the device logs every browser out of itself
    await theirs.post("/api/device/logout")
    assert (await (await them.get("/api/me")).json())["devices"] == []
    assert (await them.get(f"/api/devices/{their_id}/shot")).status == 404

    # logging out ends the session
    await me.post("/api/logout", json={})
    assert (await me.get("/api/me")).status == 401

    print("all good")
    for c in (mine, theirs, me, them, anon):
        await c.close()
    await server.close()
    shutil.rmtree(folder)


asyncio.run(main())
