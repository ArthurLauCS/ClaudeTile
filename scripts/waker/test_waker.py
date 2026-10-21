import json
import os
import tempfile
import unittest
from unittest.mock import patch

import waker


class WakerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = patch.object(waker, "ROOT", self.temp.name)
        self.root.start()
        self.now = waker.iso_seconds("2026-10-21T07:00:01+08:00")
        waker.write_json("config.json", {"token": "sk-ant-oat03-test",
                                       "enabled": True, "expected_egress_ip": "198.51.100.1"})

    def tearDown(self):
        self.root.stop()
        self.temp.cleanup()

    def test_beijing_schedule_and_no_late_catchup(self):
        seven, noon = self.now - 1, self.now - 1 + 5 * 3600 + 60
        self.assertEqual(waker.slot(self.now), seven)
        self.assertIsNone(waker.slot(self.now - 2))
        self.assertIsNone(waker.slot(self.now + 300))
        self.assertIsNone(waker.slot(noon - 1))
        self.assertEqual(waker.slot(noon + 299), noon)
        self.assertIsNone(waker.slot(seven + 20 * 3600 + 60))  # 03:01: none, so 07:00 stays fixed
        self.assertEqual(waker.next_at(self.now), noon)
        self.assertEqual(waker.next_at(self.now - 2), seven)
        self.assertEqual(waker.next_at(noon), seven + 10 * 3600 + 120)  # 17:02
        self.assertEqual(waker.next_at(seven + 10 * 3600 + 120), seven + 15 * 3600 + 180)  # 22:03
        self.assertEqual(waker.next_at(seven + 15 * 3600 + 180), seven + 86400)  # after 22:03
        with patch.object(waker, "command") as request:
            self.assertEqual(waker.wake(self.now + 3600), "outside_window")
            request.assert_not_called()

    def test_missing_token_or_paused_never_requests(self):
        for config, result in [({}, "waiting_token"), ({"enabled": False}, "paused")]:
            waker.write_json("config.json", config)
            with patch.object(waker, "command") as request:
                self.assertEqual(waker.wake(self.now), result)
                request.assert_not_called()

    def test_active_window_does_not_claim_reset(self):
        reset = self.now + 7200
        with patch.object(waker, "egress", return_value="198.51.100.1"), \
             patch.object(waker, "usage", return_value=reset), patch.object(waker, "command") as cli:
            self.assertEqual(waker.wake(self.now), "active_window")
            cli.assert_not_called()
        self.assertEqual(waker.status(self.now)["state"]["resets_at"], reset)

    def test_proxy_failure_never_sends_credentials(self):
        with patch.object(waker, "egress", return_value="203.0.113.1"), \
             patch.object(waker, "usage") as request:
            self.assertEqual(waker.wake(self.now), "egress_mismatch")
            request.assert_not_called()

    def test_inference_only_token_and_idempotency(self):
        output = json.dumps({"type": "result", "subtype": "success", "is_error": False})
        with patch.object(waker, "egress", return_value="198.51.100.1"), \
             patch.object(waker, "usage", return_value=None), \
             patch.object(waker, "command", return_value=(0, output)) as cli:
            self.assertEqual(waker.wake(self.now), "sent")
            self.assertEqual(waker.wake(self.now + 60), "already_done")
            self.assertEqual(cli.call_count, 1)
            self.assertEqual(waker.wake(self.now + 5 * 3600 + 60), "sent")  # 12:01 is a new run
        state = waker.status(self.now)["state"]
        self.assertIsNone(state["resets_at"])
        self.assertNotIn("sk-ant", json.dumps(waker.status(self.now)))

    def test_rate_limited_usage_still_wakes(self):
        output = json.dumps({"type": "result", "subtype": "success", "is_error": False})
        with patch.object(waker, "egress", return_value="198.51.100.1"), \
             patch.object(waker, "oauth_request", return_value=("429", "")), \
             patch.object(waker, "command", return_value=(0, output)) as cli:
            self.assertEqual(waker.wake(self.now), "sent")
            self.assertEqual(cli.call_count, 1)

    def test_failure_retries_without_false_success(self):
        with patch.object(waker, "egress", return_value="198.51.100.1"), \
             patch.object(waker, "usage", return_value=0), \
             patch.object(waker, "command", return_value=(0, '{"type":"result","is_error":true}')) as cli:
            self.assertEqual(waker.wake(self.now), "request_failed")
            self.assertEqual(waker.wake(self.now + 60), "request_failed")
            self.assertEqual(cli.call_count, 2)

    def test_usage_formats_and_timezone(self):
        reset = "2026-10-20T04:00:00.337986-07:00"
        self.assertEqual(waker.session_reset({"five_hour": {"resets_at": reset}}),
                         waker.iso_seconds("2026-10-20T19:00:00+08:00"))
        self.assertEqual(waker.session_reset({"limits": [{"kind": "session", "resets_at": None}]}), 0)
        with self.assertRaises(ValueError):
            waker.session_reset({"error": "unrecognized"})

    def test_credentials_never_inherited(self):
        with patch.dict(os.environ, {"ANTHROPIC_API_KEY": "secret", "NO_PROXY": "*",
                                     "ANTHROPIC_BASE_URL": "https://other.example"}):
            env = waker.environment("sk-ant-oat03-test")
        self.assertNotIn("ANTHROPIC_API_KEY", env)
        self.assertNotIn("ANTHROPIC_BASE_URL", env)
        self.assertEqual(env["NO_PROXY"], "")
        self.assertEqual(env["HTTPS_PROXY"], waker.PROXY)
        self.assertFalse(waker.token_valid('sk-ant-oat03-test"\nheader="bad'))

    def test_configure_saves_token_without_sending_a_request(self):
        with patch.object(waker, "input_line", side_effect=["1", "0"]), \
             patch.object(waker.getpass, "getpass", return_value="sk-ant-oat03-new"), \
             patch.object(waker, "command") as request, patch("builtins.print"):
            waker.configure()
            request.assert_not_called()
        config = waker.read_json("config.json")
        # The single-token config becomes a list; adding keeps the old token.
        self.assertEqual(config["tokens"], ["sk-ant-oat03-test", "sk-ant-oat03-new"])
        self.assertNotIn("token", config)
        self.assertEqual(config["expected_egress_ip"], "198.51.100.1")
        self.assertNotIn("sk-ant", json.dumps(waker.status(self.now)))
        self.assertEqual([t["tail"] for t in waker.status(self.now)["tokens"]], ["test", "-new"])
        self.assertEqual(waker.status(self.now)["token_tail"], "test")

    def test_remove_token_by_number(self):
        waker.write_json("config.json", {"tokens": ["sk-ant-oat03-aaaa", "sk-ant-oat03-bbbb"], "enabled": True})
        with patch.object(waker, "input_line", side_effect=["4", "3", "4", "1", "0"]), patch("builtins.print"):
            waker.configure()
        self.assertEqual(waker.read_json("config.json")["tokens"], ["sk-ant-oat03-bbbb"])

    def test_each_token_wakes_once_and_fails_alone(self):
        waker.write_json("config.json", {"tokens": ["sk-ant-oat03-aaaa", "sk-ant-oat03-bbbb"],
                                         "enabled": True, "expected_egress_ip": "198.51.100.1"})
        ok = json.dumps({"type": "result", "subtype": "success", "is_error": False})
        with patch.object(waker, "egress", return_value="198.51.100.1"), \
             patch.object(waker, "usage", return_value=None), \
             patch.object(waker, "command", side_effect=[(0, ok), (1, ""), (0, ok)]) as cli:
            self.assertEqual(waker.wake(self.now), "sent,request_failed")
            # Only the failed token is retried in the next cron minute.
            self.assertEqual(waker.wake(self.now + 60), "sent")
            self.assertEqual(waker.wake(self.now + 120), "already_done")
            self.assertEqual(cli.call_count, 3)
            self.assertIn("sk-ant-oat03-bbbb", cli.call_args[0][1]["CLAUDE_CODE_OAUTH_TOKEN"])
        self.assertEqual([t["state"]["result"] for t in waker.status(self.now)["tokens"]], ["sent", "sent"])

    def test_same_token_reports_unchanged(self):
        with patch.object(waker, "input_line", side_effect=["1", "0"]), \
             patch.object(waker.getpass, "getpass", return_value="sk-ant-oat03-test"), \
             patch.object(waker, "write_json") as write, patch("builtins.print") as out:
            waker.configure()
            write.assert_not_called()
        self.assertIn("nothing changed", " ".join(str(c) for c in out.call_args_list))

    def test_profile_returns_only_email_and_subscription(self):
        body = {"account": {"email": "test@example.com", "uuid": "private"},
                "organization": {"organization_type": "claude_max", "rate_limit_tier": "default_claude_max_5x"}}
        usage = {"five_hour": {"utilization": 20, "resets_at": "2026-10-20T19:00:00+08:00"},
                 "seven_day": {"utilization": 53, "resets_at": None}, "secret": "not-returned"}
        with patch.object(waker, "egress", return_value="198.51.100.1"), \
             patch.object(waker, "command", side_effect=[(0, json.dumps(body) + "\n200"), (0, json.dumps(usage) + "\n200")]) as request:
            result = waker.profile()["accounts"][0]
            self.assertEqual(result["tail"], "test")
            self.assertEqual(result["email"], "test@example.com")
            self.assertEqual(result["plan"], "Max 5x")
            self.assertEqual(result["usage"]["limits"][0]["percent"], 20)
            self.assertEqual(result["usage"]["limits"][1]["kind"], "weekly_all")
            self.assertNotIn("secret", json.dumps(result))
            self.assertNotIn("private", json.dumps(result))
            args = request.call_args[0]
            self.assertNotIn("sk-ant", " ".join(args[0]))
            self.assertNotIn("CLAUDE_CODE_OAUTH_TOKEN", args[1])
            self.assertIn(b"Authorization: Bearer sk-ant-oat03-test", args[2])
        body["organization"]["plan_display_name"] = "Max 20x"
        self.assertEqual(waker.parse_profile(body)["plan"], "Max 20x")

    def test_profile_scope_and_proxy_failures_never_guess_identity(self):
        with patch.object(waker, "egress", return_value="198.51.100.1"), \
             patch.object(waker, "command", return_value=(0, "{}\n403")):
            self.assertEqual(waker.profile(), {"result": "ok", "accounts": [
                {"result": "no_profile_scope", "usage_status": "403", "tail": "test"}]})
        with patch.object(waker, "egress", return_value="203.0.113.1"), patch.object(waker, "command") as request:
            self.assertEqual(waker.profile(), {"result": "egress_mismatch", "accounts": []})
            request.assert_not_called()
        waker.write_json("config.json", {})
        with patch.object(waker, "command") as request:
            self.assertEqual(waker.profile(), {"result": "waiting_token", "accounts": []})
            request.assert_not_called()

if __name__ == "__main__":
    unittest.main()
