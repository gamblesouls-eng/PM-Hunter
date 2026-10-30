#!/bin/zsh
# Publishes the website on poopihunt.xyz: the API (server/hunt_server.py), the
# battle page (site/app/, with its fonts) and the admins' logs page (site/app/logs/). Runs as the poopihunt user, in its own folder
# on the stingray server; nothing of stingray-digital.online (relay) is touched.
#   ./publish-server.sh          update the API and the page
#   ./publish-server.sh --setup  the first time: folders, Python, the service,
#                                the nginx site (checked with nginx -t first)
set -e
cd "$(dirname "$0")"
SERVER=root@153.92.4.40
KEY=~/.ssh/clodsire_vps
BASE=/srv/poopihunt.xyz
ssh_() { ssh -i $KEY $SERVER "$@"; }

if [[ $1 == --setup ]]; then
  ssh_ "set -e
    install -d -o poopihunt -g poopihunt -m 755 $BASE/api $BASE/public/app
    install -d -o poopihunt -g poopihunt -m 700 $BASE/data
    [ -x $BASE/api/.venv/bin/python ] || sudo -u poopihunt python3 -m venv $BASE/api/.venv
    sudo -u poopihunt $BASE/api/.venv/bin/pip install -q --no-cache-dir --disable-pip-version-check 'aiohttp==3.14.5'"
  scp -q -i $KEY server/poopihunt-api.service $SERVER:/etc/systemd/system/poopihunt-api.service
  scp -q -i $KEY server/nginx-poopihunt.xyz $SERVER:/tmp/poopihunt.xyz.new
  # the new nginx site only if nginx accepts it; else the old one goes back
  ssh_ "set -e
    cp /etc/nginx/sites-available/poopihunt.xyz /root/poopihunt.xyz.nginx.bak
    mv /tmp/poopihunt.xyz.new /etc/nginx/sites-available/poopihunt.xyz
    if nginx -t 2>/tmp/nginx-test.txt; then systemctl reload nginx; else
      cp /root/poopihunt.xyz.nginx.bak /etc/nginx/sites-available/poopihunt.xyz; cat /tmp/nginx-test.txt; exit 1; fi
    systemctl daemon-reload
    systemctl enable -q poopihunt-api"
fi

scp -q -i $KEY server/hunt_server.py $SERVER:$BASE/api/hunt_server.py
ssh_ "install -d -o poopihunt -g poopihunt -m 755 $BASE/public/app/fonts $BASE/public/app/logs"
scp -q -i $KEY site/app/index.html site/app/manifest.json site/app/icon-*.png $SERVER:$BASE/public/app/
scp -q -i $KEY site/app/fonts/*.woff2 $SERVER:$BASE/public/app/fonts/
scp -q -i $KEY site/app/logs/index.html $SERVER:$BASE/public/app/logs/
ssh_ "set -e
  chown poopihunt:poopihunt $BASE/api/hunt_server.py $BASE/public/app/index.html $BASE/public/app/manifest.json $BASE/public/app/icon-*.png $BASE/public/app/fonts/* $BASE/public/app/logs/index.html
  chmod 644 $BASE/api/hunt_server.py $BASE/public/app/index.html $BASE/public/app/manifest.json $BASE/public/app/icon-*.png $BASE/public/app/fonts/* $BASE/public/app/logs/index.html
  systemctl restart poopihunt-api
  sleep 1
  systemctl is-active poopihunt-api"
# the download page, filled in with the build that's up (publish-android.sh and publish-windows.sh do the same)
got=$(mktemp -d)
ssh_ "cat $BASE/public/version.json" > $got/version.json
ssh_ "cat $BASE/public/windows.json 2>/dev/null" > $got/windows.json || true
ssh_ "cat $BASE/public/mac.json 2>/dev/null" > $got/mac.json || true
page=$got/index.html
python3 site/fill_page.py $got/version.json $got/windows.json $got/mac.json > $page
scp -q -i $KEY $page $SERVER:$BASE/public/index.html
ssh_ "chown poopihunt:poopihunt $BASE/public/index.html && chmod 644 $BASE/public/index.html"
echo "published on https://poopihunt.xyz/app"
