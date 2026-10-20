#!/bin/sh
# Run as root from this directory, AFTER providing the CLI and proxy.json.
set -eu
test "$(id -u)" = 0
test -x /opt/claudetile-waker/claude
test -f /etc/claudetile-waker/proxy.json
test -x /usr/bin/python
test -x /usr/local/bin/xray
id claudetile-waker >/dev/null 2>&1 || useradd --system --home-dir /var/lib/claudetile-waker --shell /bin/bash claudetile-waker
install -d -m 700 -o claudetile-waker -g claudetile-waker /var/lib/claudetile-waker
install -d -m 755 /opt/claudetile-waker
install -m 644 waker.py /opt/claudetile-waker/waker.py
chown root:claudetile-waker /etc/claudetile-waker/proxy.json
chmod 640 /etc/claudetile-waker/proxy.json
/usr/local/bin/xray run -test -config /etc/claudetile-waker/proxy.json
install -m 644 claudetile-waker-proxy.service /etc/systemd/system/claudetile-waker-proxy.service
systemctl daemon-reload
systemctl enable claudetile-waker-proxy.service
systemctl restart claudetile-waker-proxy.service
if ! test -f /var/lib/claudetile-waker/config.json; then
    # No token is imported from the administrator's Claude login.
    # Old curl does not retry ECONNREFUSED; wait for the new service's listener.
    attempt=0
    until ip=$(curl -fsS --noproxy '' --proxy http://127.0.0.1:17898 --connect-timeout 8 --max-time 20 https://api.ipify.org); do
        attempt=$((attempt + 1))
        test "$attempt" -lt 5 || exit 1
        sleep 1
    done
    CT_EXPECTED_IP="$ip" /usr/bin/python - <<'PY'
import json, os
path = "/var/lib/claudetile-waker/config.json"
fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, "w") as f:
    json.dump({"enabled": True, "token": "", "expected_egress_ip": os.environ["CT_EXPECTED_IP"]}, f)
PY
    chown claudetile-waker:claudetile-waker /var/lib/claudetile-waker/config.json
fi
# cronie (crond) honours CRON_TZ; Debian/Ubuntu cron ignores it and needs an Asia/Shanghai system timezone.
systemctl is-active crond >/dev/null 2>&1 || systemctl is-active cron >/dev/null
install -m 644 claudetile-waker.cron /etc/cron.d/claudetile-waker
runuser -u claudetile-waker -- /usr/bin/python /opt/claudetile-waker/waker.py status
