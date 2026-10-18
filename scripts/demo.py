"""README 截图用的假数据：会话、对话记录（标题和工具）、半年的用量历史、额度。

用法：python scripts/demo.py <输出目录> <pid,pid,pid,pid> <zh|en>
四个 pid 借用正在运行的进程（会话文件要指向活着的进程才会显示）。生成后这样启动：
claudetile.exe --claude-dir <输出目录>/claude-<语言> --usage-file <输出目录>/usage.json --data <输出目录>/data-<语言> --dark
截图时在磁贴下面垫一张亮色背景，不要让终端等真实窗口出现在截图里。
"""
import json
import random
import sys
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

out, pids, lang = Path(sys.argv[1]), [int(x) for x in sys.argv[2].split(",")], sys.argv[3]
claude, data = out / f"claude-{lang}", out / f"data-{lang}"
for d in (claude / "sessions", claude / "projects", data):
    d.mkdir(parents=True, exist_ok=True)
for f in (claude / "sessions").glob("*.json"):
    f.unlink()

now = int(time.time() * 1000)
zh = lang == "zh"
sessions = [
    ("api-server", "busy", None, 4, "接入支付回调" if zh else "Add payment webhooks", "Bash", {"description": "Run integration tests"}),
    ("web-app", "waiting", "dialog open", 1, "修复登录页布局" if zh else "Fix login page layout", "Edit", {"file_path": "C:\\code\\web-app\\src\\Login.tsx"}),
    ("mobile", "busy", None, 12, "离线缓存重构" if zh else "Refactor offline cache", "Grep", {"pattern": "CacheStore"}),
    ("docs", "idle", None, 38, "整理发布说明" if zh else "Draft release notes", "Read", {"file_path": "C:\\code\\docs\\CHANGELOG.md"}),
]
for i, (proj, status, wf, mins, title, tool, inp) in enumerate(sessions):
    sid = f"demo-{lang}-{i}"
    s = {"pid": pids[i], "sessionId": sid, "cwd": f"C:\\code\\{proj}", "entrypoint": "claude-desktop" if i == 2 else "cli",
         "name": f"{proj}-0{i}", "status": status, "updatedAt": now, "statusUpdatedAt": now - mins * 60000}
    if wf:
        s["waitingFor"] = wf
    (claude / "sessions" / f"{pids[i]}.json").write_text(json.dumps(s), encoding="utf-8")
    pdir = claude / "projects" / proj
    pdir.mkdir(exist_ok=True)
    ts = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.000Z")
    lines = [
        {"type": "ai-title", "aiTitle": title, "sessionId": sid},
        {"type": "assistant", "sessionId": sid, "cwd": f"C:\\code\\{proj}", "timestamp": ts,
         "message": {"id": f"msg-{sid}", "model": "claude-opus-5-5", "content": [{"type": "tool_use", "name": tool, "input": inp}],
                     "usage": {"input_tokens": 1, "output_tokens": 1, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}},
    ]
    (pdir / f"{sid}.jsonl").write_text("".join(json.dumps(l, ensure_ascii=False) + "\n" for l in lines), encoding="utf-8")

# 半年的每日用量：工作日多、周末少，最近几周更忙
random.seed(7)
today = datetime.now().date()
epoch = datetime(1970, 1, 1).date()
rows = []
projects = [("api-server", 1.0), ("web-app", 0.7), ("mobile", 0.45), ("docs", 0.2)]
models = [("claude-opus-5-5", 0.75), ("claude-sonnet-5-5", 0.2), ("claude-haiku-5-5", 0.05)]
for back in range(182):
    d = today - timedelta(days=back)
    if back > 0 and random.random() < (0.45 if d.weekday() >= 5 else 0.08):
        continue
    base = random.uniform(0.6, 4.5) * (1.6 if back < 28 else 1.0) * (0.35 if d.weekday() >= 5 else 1.0)
    for p, pw in projects:
        if back > 0 and random.random() < 0.3:
            continue
        for m, mw in models:
            t = int(base * pw * mw * 1e6)
            rows.append(f"{(d - epoch).days}|{p}|{m}|{t // 50}|{t // 4}|{t - t // 50 - t // 4}|{t * 20}")
(data / "history.txt").write_text("\n".join(rows) + "\n", encoding="utf-8")
(data / "config.txt").write_text(f"lang|{lang}\ntopmost|1\n", encoding="utf-8")

resets = lambda h: (datetime.now(timezone.utc) + timedelta(hours=h)).strftime("%Y-%m-%dT%H:%M:%SZ")
usage = {"limits": [
    {"kind": "session", "percent": 46, "resets_at": resets(2.3)},
    {"kind": "weekly_all", "percent": 31, "resets_at": resets(4 * 24 + 6)},
    {"kind": "weekly_scoped", "percent": 18, "resets_at": resets(4 * 24 + 6), "scope": {"model": {"display_name": "Opus"}}},
]}
(out / "usage.json").write_text(json.dumps(usage), encoding="utf-8")
print("ok")
