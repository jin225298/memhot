import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import time
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location("maintenance_queue", Path(__file__).with_name("maintenance-queue.py"))
maintenance = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(maintenance)


class QueueMaintenanceTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.database = self.root / "test.sqlite"
        self.receipt = self.root / "receipt.json"
        self.conn = sqlite3.connect(self.database)
        self.conn.executescript("""
            CREATE TABLE jobs(id TEXT PRIMARY KEY,status TEXT,next_at INTEGER,user_id TEXT DEFAULT 'owner');
            CREATE TABLE billing_usage(state TEXT,user_id TEXT,operation_key TEXT);
            CREATE TABLE receipts(status TEXT,user_id TEXT,job_id TEXT,key TEXT,created INTEGER);
            CREATE TABLE public_model_calls(status TEXT);
            CREATE TABLE records(kind TEXT,data TEXT);
        """)
        self.conn.commit()

    def tearDown(self):
        self.conn.close()
        self.tmp.cleanup()

    def insert(self, job, status="queued", next_at=0):
        self.conn.execute("INSERT INTO jobs(id,status,next_at) VALUES(?,?,?)", (job,status,next_at))
        self.conn.commit()

    def next_at(self, job):
        return self.conn.execute("SELECT next_at FROM jobs WHERE id=?",(job,)).fetchone()[0]

    def test_hold_and_release_preserve_running_and_original_queue_times(self):
        self.insert("queued",next_at=123)
        self.insert("running","running",456)
        self.insert("blocked","blocked",789)
        result = maintenance.hold(self.database,self.receipt,900)
        deadline = result["deadline"]
        self.assertEqual(self.next_at("queued"),deadline)
        self.assertEqual(self.next_at("running"),456)
        self.assertEqual(self.next_at("blocked"),789)
        self.assertEqual(os.stat(self.receipt).st_mode & 0o777,0o600)
        self.assertEqual(maintenance.release(self.database,self.receipt)["restored"],1)
        self.assertEqual(self.next_at("queued"),123)
        self.assertEqual(self.conn.execute("SELECT status FROM jobs WHERE id='queued'").fetchone()[0],"queued")

    def test_new_fanout_and_running_retry_are_held_but_keep_the_retry_delay(self):
        self.insert("running","running")
        deadline = maintenance.hold(self.database,self.receipt,900)["deadline"]
        self.insert("fanout")
        retry_at = int(time.time())+60
        self.conn.execute("UPDATE jobs SET status='queued',next_at=? WHERE id='running'",(retry_at,))
        self.conn.commit()
        self.assertEqual(self.next_at("fanout"),deadline)
        self.assertEqual(self.next_at("running"),deadline)
        maintenance.release(self.database,self.receipt)
        self.assertEqual(self.next_at("fanout"),0)
        self.assertEqual(self.next_at("running"),retry_at)

    def test_release_preserves_delays_changed_after_hold(self):
        self.insert("short")
        self.insert("long")
        deadline = maintenance.hold(self.database,self.receipt,900)["deadline"]
        later_retry = int(time.time())+120
        self.conn.execute("UPDATE jobs SET next_at=? WHERE id='short'",(later_retry,))
        self.conn.execute("UPDATE jobs SET next_at=? WHERE id='long'",(deadline+3600,))
        self.conn.commit()
        maintenance.release(self.database,self.receipt)
        self.assertEqual(self.next_at("short"),later_retry)
        self.assertEqual(self.next_at("long"),deadline+3600)

    def test_status_counts_direct_pending_requests_and_never_returns_content(self):
        self.conn.executescript("""
            INSERT INTO billing_usage(state) VALUES('reserved');
            INSERT INTO receipts(status) VALUES('pending');
            INSERT INTO public_model_calls VALUES('pending');
            INSERT INTO records VALUES('discussion_request','{"status":"pending","text":"PRIVATE"}');
        """)
        self.insert("running","running")
        result = maintenance.status(self.database)
        for key in ["runningJobs","reservedUsage","pendingReceipts","pendingPublicCalls","pendingDiscussions"]:
            self.assertEqual(result[key],1)
        self.assertFalse(result["quiescent"])
        self.assertNotIn("PRIVATE",json.dumps(result))

    def test_hold_is_bounded_and_conflicting_receipts_cannot_replace_it(self):
        for ttl in [0,3601]:
            with self.assertRaises(ValueError):
                maintenance.hold(self.database,self.receipt,ttl)
        self.insert("queued")
        deadline = maintenance.hold(self.database,self.receipt,1)["deadline"]
        with self.assertRaises(ValueError):
            maintenance.hold(self.database,self.root/"other.json",900)
        # Even if the operator cannot restore, the old server can claim after the bounded deadline.
        self.assertEqual(self.conn.execute("SELECT count(*) FROM jobs WHERE status='queued' AND next_at<=?",(deadline+1,)).fetchone()[0],1)

    def test_historical_unknown_discussions_stay_preserved_without_blocking_a_drained_process(self):
        self.conn.execute("INSERT INTO records VALUES('discussion_request',json_object('status','pending','createdAt',datetime('now','-2 hours')))")
        self.conn.commit()
        result=maintenance.status(self.database)
        self.assertEqual(result['historicalUnknownDiscussions'],1)
        self.assertEqual(result['pendingDiscussions'],0)
        self.assertTrue(result['quiescent'])
        self.assertEqual(self.conn.execute("SELECT json_extract(data,'$.status') FROM records").fetchone()[0],'pending')
        self.conn.execute("INSERT INTO records VALUES('discussion_request',json_object('status','pending','createdAt',datetime('now','-30 minutes')))")
        self.conn.commit()
        self.assertFalse(maintenance.status(self.database)['quiescent'])

    def test_unknown_receipts_require_age_terminal_job_and_matching_tenant(self):
        self.insert('old','failed')
        self.conn.execute("INSERT INTO receipts VALUES('pending','owner','old','request',unixepoch()-7200)")
        self.conn.execute("INSERT INTO billing_usage VALUES('unknown','owner','request')")
        self.conn.commit()
        result=maintenance.status(self.database)
        self.assertEqual(result['historicalUnknownReceipts'],1)
        self.assertTrue(result['quiescent'])
        for sql in [
            "UPDATE receipts SET created=unixepoch()",
            "UPDATE receipts SET created=NULL",
            "UPDATE jobs SET status='running'",
            "UPDATE jobs SET user_id='someone-else'",
            "UPDATE billing_usage SET user_id='someone-else'",
            "UPDATE billing_usage SET state='reserved'",
        ]:
            self.conn.execute(sql)
            self.conn.commit()
            self.assertFalse(maintenance.status(self.database)['quiescent'],sql)
            self.conn.executescript("UPDATE receipts SET created=unixepoch()-7200; UPDATE jobs SET status='failed',user_id='owner'; UPDATE billing_usage SET state='unknown',user_id='owner';")
        self.assertEqual(self.conn.execute('SELECT status FROM receipts').fetchone()[0],'pending')
        self.assertEqual(self.conn.execute('SELECT state FROM billing_usage').fetchone()[0],'unknown')

    def test_receipt_can_restore_using_a_new_process_connection(self):
        self.insert("queued",next_at=12)
        maintenance.hold(self.database,self.receipt,900)
        loaded = json.loads(self.receipt.read_text())
        self.assertEqual(loaded["state"],"held")
        result = maintenance.release(self.database,self.receipt)
        self.assertEqual(result["restored"],1)
        self.assertEqual(json.loads(self.receipt.read_text())["state"],"released")
        self.assertEqual(self.next_at("queued"),12)

    def test_failed_post_commit_receipt_update_preserves_recovery_identity(self):
        self.insert("queued",next_at=12)
        with mock.patch.object(maintenance.os,"replace",side_effect=OSError("write interrupted")):
            with self.assertRaises(OSError):
                maintenance.hold(self.database,self.receipt,900)
        self.assertEqual(json.loads(self.receipt.read_text())["state"],"prepared")
        self.assertEqual(maintenance.release(self.database,self.receipt)["restored"],1)
        self.assertEqual(self.next_at("queued"),12)

    def test_release_rejects_symlink_or_non_private_receipt(self):
        self.insert("queued")
        maintenance.hold(self.database,self.receipt,900)
        symlink = self.root / "linked.json"
        symlink.symlink_to(self.receipt)
        with self.assertRaises((ValueError,OSError)):
            maintenance.release(self.database,symlink)
        self.receipt.chmod(0o644)
        with self.assertRaises(ValueError):
            maintenance.release(self.database,self.receipt)
        self.receipt.chmod(0o600)
        self.assertEqual(maintenance.release(self.database,self.receipt)["restored"],1)


if __name__ == "__main__":
    unittest.main()
