#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""Daily 07:00 Asia/Shanghai wake-up. Python 2.7/3 stdlib (CentOS 7 compatible).

Inspired by weidwonder/claude-waker. Uses the official CLI directly, as the
desktop's accounts::ping does; no SDK dependency or copied login credentials.
"""
from __future__ import print_function

import calendar
import datetime
import getpass
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import time

ROOT = os.environ.get("CLAUDETILE_WAKER_DATA", "/var/lib/claudetile-waker")
CLI = "/opt/claudetile-waker/claude"
PROXY = "http://127.0.0.1:17898"
CRON = "/etc/cron.d/claudetile-waker"
OFFSET = 8 * 3600


def read_json(name):
    try:
        with open(os.path.join(ROOT, name)) as stream:
            return json.load(stream)
    except (IOError, ValueError):
        return {}


def write_json(name, value):
    fd, path = tempfile.mkstemp(prefix=".waker-", dir=ROOT)
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump(value, stream)
            stream.write("\n")
        getattr(os, "replace", os.rename)(path, os.path.join(ROOT, name))
    finally:
        if os.path.exists(path):
            os.unlink(path)


def day(now):
    return time.strftime("%Y-%m-%d", time.gmtime(now + OFFSET))


def scheduled(now):
    local = time.gmtime(now + OFFSET)
    # Short retries only. Never catch up at noon/after a reboot and shift the window.
    return local.tm_hour == 7 and local.tm_min < 5


def next_at(now):
    local_day = int((now + OFFSET) // 86400)
    target = local_day * 86400 + 7 * 3600 - OFFSET
    return target if target > now else target + 86400


def iso_seconds(value):
    match = re.match(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.\d+)?(Z|[+-]\d\d:\d\d)$", value or "")
    if not match:
        raise ValueError("Invalid reset time")
    stamp = calendar.timegm(datetime.datetime.strptime(match.group(1), "%Y-%m-%dT%H:%M:%S").timetuple())
    zone = match.group(2)
    if zone != "Z":
        offset = (int(zone[1:3]) * 60 + int(zone[4:6])) * 60
        stamp -= offset if zone[0] == "+" else -offset
    return stamp


def session_reset(usage):
    for item in usage.get("limits") or []:
        if item.get("kind") == "session":
            return iso_seconds(item["resets_at"]) if item.get("resets_at") else 0
    window = usage.get("five_hour")
    if isinstance(window, dict):
        return iso_seconds(window["resets_at"]) if window.get("resets_at") else 0
    if "five_hour" in usage and window is None:
        return 0
    raise ValueError("Unknown usage schema")


def token_valid(token):
    return bool(re.match(r"^sk-ant-oat[A-Za-z0-9_.-]+$", token or ""))


def environment(token=None):
    # Do not inherit API keys, alternate providers, base URLs or proxy bypasses.
    env = {"HOME": ROOT, "PATH": "/usr/local/bin:/usr/bin:/bin", "LANG": "en_US.UTF-8"}
    for key in ("HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"):
        env[key] = PROXY
    env.update(NO_PROXY="", no_proxy="", CLAUDE_CONFIG_DIR=os.path.join(ROOT, ".claude"),
               DISABLE_AUTOUPDATER="1", CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC="1")
    if token:
        env["CLAUDE_CODE_OAUTH_TOKEN"] = token
    return env


def command(args, env, data=b"", timeout=60):
    # Files avoid pipe deadlocks and keep prompts/errors (potential credentials)
    # out of logs. Only fixed error categories are persisted.
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        process = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=out, stderr=err,
                                   env=env, cwd=ROOT, preexec_fn=os.setsid)
        try:
            process.stdin.write(data)
            process.stdin.close()
            deadline = time.time() + timeout
            while process.poll() is None and time.time() < deadline:
                time.sleep(0.2)
            if process.poll() is None:
                raise RuntimeError("timeout")
            out.seek(0)
            return process.returncode, out.read(262144).decode("utf-8", "replace")
        finally:
            # Includes children left behind by the CLI.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except OSError:
                pass
            process.wait()


def egress():
    code, output = command(["curl", "--silent", "--show-error", "--fail", "--noproxy", "",
                            "--proxy", PROXY, "--connect-timeout", "8", "--max-time", "15",
                            "https://api.ipify.org"], environment(), timeout=20)
    ip = output.strip()
    if code or not re.match(r"^\d{1,3}(?:\.\d{1,3}){3}$", ip):
        raise RuntimeError("proxy_unavailable")
    return ip


def oauth_request(token, endpoint):
    headers = 'header = "Authorization: Bearer %s"\nheader = "anthropic-beta: oauth-2025-04-20"\n' % token
    code, output = command(["curl", "--silent", "--show-error", "--noproxy", "",
                            "--proxy", PROXY, "--connect-timeout", "8", "--max-time", "20",
                            "--config", "-", "--write-out", "\n%{http_code}",
                            "https://api.anthropic.com/api/oauth/" + endpoint],
                           environment(), headers.encode("ascii"), timeout=25)
    if code:
        raise RuntimeError("usage_network_error")
    body, status = output.rsplit("\n", 1)
    return status, body


def usage(token):
    # setup-token may lack user:profile: a 403 means "cannot inspect", NOT idle.
    status, body = oauth_request(token, "usage")
    if status == "403":
        return None
    if status != "200":
        raise RuntimeError("usage_http_" + status)
    return session_reset(json.loads(body))


def usage_snapshot(body):
    """Only return quota percentages/reset dates, never arbitrary API fields."""
    import math
    result = {"limits": []}
    source = body.get("limits") or [
        dict(kind=kind, percent=(body.get(key) or {}).get("utilization"),
             resets_at=(body.get(key) or {}).get("resets_at"))
        for key, kind in [("five_hour", "session"), ("seven_day", "weekly_all")]
    ]
    for item in source:
        kind, percent, reset = item.get("kind"), item.get("percent"), item.get("resets_at")
        if kind not in ("session", "weekly_all") or not isinstance(percent, (int, float)) or isinstance(percent, bool):
            continue
        if math.isnan(percent) or math.isinf(percent) or percent < 0:
            continue
        if reset is not None:
            iso_seconds(reset)
        result["limits"].append({"kind": kind, "percent": percent, "resets_at": reset})
    return result


def parse_profile(body):
    account, organization = body.get("account") or {}, body.get("organization") or {}
    email = account.get("email") or ""
    if not isinstance(email, type(u"")) or len(email) > 254 or not re.match(r"^[^\s@]+@[^\s@]+\.[^\s@]+$", email):
        email = ""
    plans = {"claude_max": "Max", "claude_pro": "Pro", "claude_team": "Team", "claude_enterprise": "Enterprise"}
    plan = plans.get(organization.get("organization_type"), "")
    tier = re.search(r"_(\d+x)$", organization.get("rate_limit_tier") or "")
    if plan == "Max" and tier:
        plan += " " + tier.group(1)
    # This is the same display field used by the official CLI when supplied.
    display = organization.get("plan_display_name")
    if isinstance(display, type(u"")) and 0 < len(display) <= 80 and not any(ord(c) < 32 for c in display):
        plan = display
    return {"result": "ok", "email": email, "plan": plan}


def profile():
    """Read identity on demand; never infer it from cached local accounts."""
    config = read_json("config.json")
    token = config.get("token")
    if not token_valid(token):
        return {"result": "waiting_token"}
    try:
        if egress() != config.get("expected_egress_ip"):
            return {"result": "egress_mismatch"}
        status_code, body = oauth_request(token, "profile")
        if status_code == "403":
            result = {"result": "no_profile_scope"}
        elif status_code == "401":
            return {"result": "expired"}
        elif status_code != "200":
            return {"result": "unavailable"}
        else:
            result = parse_profile(json.loads(body))
        try:
            usage_code, usage_body = oauth_request(token, "usage")
            result["usage_status"] = usage_code
            if usage_code == "200":
                result["usage"] = usage_snapshot(json.loads(usage_body))
        except (RuntimeError, ValueError, IOError, TypeError, AttributeError):
            result["usage_status"] = "unavailable"
        return result
    except (RuntimeError, ValueError, IOError, TypeError, AttributeError):
        return {"result": "unavailable"}


def cli_success(output):
    try:
        result = json.loads(output)
        return result.get("type") == "result" and result.get("is_error") is False and result.get("subtype") == "success"
    except (ValueError, AttributeError):
        return False


def record(result, now, **fields):
    state = {"result": result, "at": int(now), "day": day(now)}
    state.update(fields)
    write_json("state.json", state)
    return state


def wake(now=None):
    now = time.time() if now is None else now
    config = read_json("config.json")
    if not config.get("enabled", True):
        return "paused"
    if not token_valid(config.get("token")):
        return "waiting_token"
    if not scheduled(now):
        return "outside_window"
    previous = read_json("state.json")
    if previous.get("day") == day(now) and previous.get("result") in ("sent", "active_window"):
        return "already_done"
    try:
        actual_ip = egress()
        if actual_ip != config.get("expected_egress_ip"):
            return record("egress_mismatch", now)["result"]
        reset = usage(config["token"])
        if reset is not None and reset > now:
            return record("active_window", now, resets_at=reset)["result"]
        code, output = command([CLI, "-p", "--model", "haiku", "--effort", "low",
                                "--safe-mode", "--no-session-persistence", "--tools", "",
                                "--system-prompt", "Reply ok.", "--max-turns", "1",
                                "--output-format", "json"], environment(config["token"]), b"hi", 60)
        if code or not cli_success(output):
            return record("request_failed", now)["result"]
        # Successful model request is not proof of a 12:00 reset.
        confirmed = None
        try:
            confirmed = usage(config["token"])
        except (RuntimeError, ValueError):
            pass
        return record("sent", now, resets_at=confirmed, egress_ip=actual_ip)["result"]
    except (RuntimeError, ValueError, IOError) as error:
        reason = str(error)
        if reason not in ("timeout", "proxy_unavailable", "usage_network_error",
                          "usage_http_401", "usage_http_429", "usage_http_500", "usage_http_503"):
            reason = "request_failed"
        return record(reason, now)["result"]


def status(now=None):
    now = time.time() if now is None else now
    config = read_json("config.json")
    configured = token_valid(config.get("token"))
    # Last 4 characters only identify which token is stored; cron tells the tile the schedule exists.
    return {"configured": configured, "enabled": bool(config.get("enabled", True)),
            "token_tail": config["token"][-4:] if configured else "",
            "cron": os.path.isfile(CRON),
            "next_at": next_at(now), "state": read_json("state.json"),
            "schedule": "07:00 Asia/Shanghai", "expected_reset": "12:00 (not guaranteed)"}


def configure():
    while True:
        current = status()
        print("\nClaudeTile VPS Waker | 07:00 Beijing (UTC+8)")
        print("Token: %s | Schedule: %s" % ("..." + current["token_tail"] if current["configured"] else "MISSING",
                                            "enabled" if current["enabled"] else "paused"))
        print("1. Set OAuth token (hidden input)\n2. Pause\n3. Resume\n0. Exit")
        try:
            choice = input_line("Choose: ").strip()
        except (EOFError, KeyboardInterrupt):
            return
        config = read_json("config.json")
        if choice == "0":
            return
        if choice == "1":
            token = getpass.getpass("Paste the token from claude setup-token: ").strip()
            if not token_valid(token):
                print("Invalid OAuth token format; no change saved.")
                continue
            if token == config.get("token") and config.get("enabled", True):
                print("This token (...%s) is already saved and enabled; nothing changed." % token[-4:])
                continue
            config["token"] = token
            config["enabled"] = True
        elif choice in ("2", "3"):
            config["enabled"] = choice == "3"
        else:
            continue
        write_json("config.json", config)
        print("Saved. No model request was sent. Next scheduled run is at 07:00 Beijing.")


try:
    input_line = raw_input
except NameError:
    input_line = input


def main():
    action = sys.argv[1] if len(sys.argv) > 1 else "status"
    if action == "ssh":
        action = os.environ.get("SSH_ORIGINAL_COMMAND", "")
        if action not in ("status", "configure", "profile"):
            raise SystemExit("Only status, configure and profile are allowed.")
    if action == "status":
        print(json.dumps(status()))
    elif action == "configure":
        configure()
    elif action == "profile":
        print(json.dumps(profile()))
    elif action == "wake":
        import fcntl
        with open(os.path.join(ROOT, "wake.lock"), "a") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except IOError:
                return
            print(wake())
    else:
        raise SystemExit("Usage: waker.py status|configure|profile|wake")


if __name__ == "__main__":
    main()
