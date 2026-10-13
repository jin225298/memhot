#!/usr/bin/env python3
"""Bounded queue drain for deployment; never cancels running or paid requests.

Run as root. Use a private receipt directory (0700). The durable SQLite hold
tables are authoritative for jobs inserted or retried after the receipt is saved.
The old server resumes claiming automatically when the hold deadline expires.
"""
from contextlib import closing
import argparse
import json
import os
from pathlib import Path
import sqlite3
import stat
import tempfile
import time
import uuid

TRIGGERS = ("memhot_maintenance_queue_insert", "memhot_maintenance_queue_update")
# An unknown billing outcome is a durable retry fence, not a live request. Only
# count it as historical after an hour and a terminal, same-tenant job. Callers
# must still close API admission and wait for old HTTP workers before stopping.
HISTORICAL_RECEIPT = """r.created < unixepoch()-3600
    AND EXISTS(SELECT 1 FROM billing_usage b WHERE b.user_id=r.user_id
        AND b.operation_key=r.key AND b.state='unknown')
    AND EXISTS(SELECT 1 FROM jobs j WHERE j.user_id=r.user_id
        AND j.id=r.job_id AND j.status IN ('failed','completed'))"""


def connect(database):
    path = Path(database).resolve(strict=True)
    conn = sqlite3.connect(f"file:{path}?mode=rw", uri=True, timeout=30, isolation_level=None)
    conn.row_factory = sqlite3.Row
    return conn


def write_receipt(path, value, exclusive=False):
    path = Path(path)
    temporary = None
    if exclusive:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    else:
        read_receipt(path)
        fd, temporary = tempfile.mkstemp(prefix=".queue-receipt-",dir=path.parent)
    try:
        with os.fdopen(fd, "w") as file:
            json.dump(value, file, separators=(",", ":"))
            file.flush()
            os.fsync(file.fileno())
        if temporary:
            os.replace(temporary,path)
        directory = os.open(path.parent,os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if temporary and os.path.exists(temporary):
            os.unlink(temporary)


def read_receipt(path):
    fd = os.open(path,os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd) as file:
        info = os.fstat(file.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid() or info.st_mode & 0o077:
            raise ValueError("receipt must be a private regular file owned by the current user")
        return json.load(file)


def status(database):
    with closing(connect(database)) as conn:
        queries = {
            "runningJobs": "SELECT count(*) FROM jobs WHERE status='running'",
            "queuedJobs": "SELECT count(*) FROM jobs WHERE status='queued'",
            "reservedUsage": "SELECT count(*) FROM billing_usage WHERE state='reserved'",
            "pendingReceipts": f"SELECT count(*) FROM receipts r WHERE status='pending' AND NOT COALESCE(({HISTORICAL_RECEIPT}),0)",
            "pendingPublicCalls": "SELECT count(*) FROM public_model_calls WHERE status='pending'",
            # A pending discussion also fences an unknown historical outcome. Those
            # records survive the <= 12-minute handler and must not be replayed or deleted.
            # Only after API admission closes and old Nginx workers exit can these
            # conservative one-hour-old markers be ignored for process draining.
            "pendingDiscussions": "SELECT count(*) FROM records WHERE kind='discussion_request' AND json_extract(data,'$.status')='pending' AND COALESCE(unixepoch(json_extract(data,'$.createdAt'))>=unixepoch()-3600,1)",
        }
        result = {key: conn.execute(sql).fetchone()[0] for key, sql in queries.items()}
        result["historicalUnknownReceipts"] = conn.execute(f"SELECT count(*) FROM receipts r WHERE status='pending' AND ({HISTORICAL_RECEIPT})").fetchone()[0]
        result["historicalUnknownDiscussions"] = conn.execute("SELECT count(*) FROM records WHERE kind='discussion_request' AND json_extract(data,'$.status')='pending' AND unixepoch(json_extract(data,'$.createdAt'))<unixepoch()-3600").fetchone()[0]
        held = conn.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='memhot_maintenance_queue_hold'").fetchone()
        result["holdDeadline"] = conn.execute("SELECT MAX(deadline) FROM memhot_maintenance_queue_hold").fetchone()[0] if held else None
        result["holdActive"] = (result["holdDeadline"] or 0) > int(time.time())
        result["quiescent"] = not any(result[key] for key in queries if key != "queuedJobs")
        return result


def hold(database, receipt, ttl):
    if not 1 <= ttl <= 3600:
        raise ValueError("hold TTL must be 1 to 3600 seconds")
    receipt = Path(receipt).absolute()
    parent = receipt.parent.stat()
    if parent.st_uid != os.geteuid() or parent.st_mode & 0o077:
        raise ValueError("receipt directory must be private and owned by the current user (0700)")
    created = int(time.time())
    value = {"version": 1, "holdId": str(uuid.uuid4()), "database": str(Path(database).resolve(strict=True)), "created": created, "deadline": created + ttl, "state": "prepared"}
    with closing(connect(database)) as conn:
        conn.execute("BEGIN IMMEDIATE")
        try:
            conn.execute("CREATE TABLE IF NOT EXISTS memhot_maintenance_queue_hold(id TEXT PRIMARY KEY,created INTEGER NOT NULL,deadline INTEGER NOT NULL)")
            conn.execute("CREATE TABLE IF NOT EXISTS memhot_maintenance_queue_jobs(job_id TEXT PRIMARY KEY,original_next_at INTEGER NOT NULL,held_next_at INTEGER NOT NULL)")
            if conn.execute("SELECT count(*) FROM memhot_maintenance_queue_hold").fetchone()[0]:
                raise ValueError("an existing hold must be released with its original receipt first")
            write_receipt(receipt, value, exclusive=True)
            conn.execute("INSERT INTO memhot_maintenance_queue_hold VALUES(?,?,?)", (value["holdId"], created, value["deadline"]))
            for name, event in zip(TRIGGERS, ("INSERT", "UPDATE OF status,next_at")):
                conn.execute(f"""CREATE TRIGGER {name} AFTER {event} ON jobs
                    WHEN NEW.status='queued' AND NEW.next_at<(SELECT deadline FROM memhot_maintenance_queue_hold)
                    AND unixepoch()<(SELECT deadline FROM memhot_maintenance_queue_hold)
                    BEGIN
                      INSERT INTO memhot_maintenance_queue_jobs VALUES(NEW.id,NEW.next_at,(SELECT deadline FROM memhot_maintenance_queue_hold))
                      ON CONFLICT(job_id) DO UPDATE SET original_next_at=MAX(original_next_at,excluded.original_next_at);
                      UPDATE jobs SET next_at=(SELECT deadline FROM memhot_maintenance_queue_hold) WHERE id=NEW.id;
                    END""")
            conn.execute("INSERT INTO memhot_maintenance_queue_jobs SELECT id,next_at,? FROM jobs WHERE status='queued' AND next_at<?", (value["deadline"], value["deadline"]))
            conn.execute("UPDATE jobs SET next_at=? WHERE status='queued' AND next_at<?", (value["deadline"], value["deadline"]))
            value["initialJobs"] = [dict(row) for row in conn.execute("SELECT job_id,original_next_at,held_next_at FROM memhot_maintenance_queue_jobs ORDER BY job_id")]
            conn.commit()
        except BaseException:
            conn.rollback()
            raise
    value["state"] = "held"
    write_receipt(receipt, value)
    return {"deadline": value["deadline"], "heldJobs": len(value["initialJobs"])}


def release(database, receipt):
    receipt = Path(receipt).absolute()
    value = read_receipt(receipt)
    if value.get("database") != str(Path(database).resolve(strict=True)):
        raise ValueError("receipt belongs to a different database")
    with closing(connect(database)) as conn:
        conn.execute("BEGIN IMMEDIATE")
        try:
            active = conn.execute("SELECT id FROM memhot_maintenance_queue_hold").fetchone()
            if not active or active["id"] != value.get("holdId"):
                raise ValueError("receipt does not identify the active hold")
            for name in TRIGGERS:
                conn.execute(f"DROP TRIGGER {name}")
            changed = conn.execute("""UPDATE jobs SET next_at=(SELECT original_next_at FROM memhot_maintenance_queue_jobs h WHERE h.job_id=jobs.id)
                WHERE status='queued' AND EXISTS(SELECT 1 FROM memhot_maintenance_queue_jobs h WHERE h.job_id=jobs.id AND h.held_next_at=jobs.next_at)""").rowcount
            conn.execute("DELETE FROM memhot_maintenance_queue_jobs")
            conn.execute("DELETE FROM memhot_maintenance_queue_hold")
            conn.commit()
        except BaseException:
            conn.rollback()
            raise
    value["state"] = "released"
    value["releasedAt"] = int(time.time())
    write_receipt(receipt, value)
    return {"restored": changed}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("hold", "status", "release"))
    parser.add_argument("--database", required=True)
    parser.add_argument("--receipt")
    parser.add_argument("--ttl", type=int, default=900)
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("run as root on the server")
    if args.action != "status" and not args.receipt:
        parser.error("hold and release require --receipt")
    try:
        result = status(args.database) if args.action == "status" else hold(args.database, args.receipt, args.ttl) if args.action == "hold" else release(args.database, args.receipt)
    except (ValueError, OSError, sqlite3.Error):
        parser.exit(1, "Queue maintenance failed; inspect the private receipt and hold status before retrying.\n")
    print(json.dumps(result, separators=(",", ":")))


if __name__ == "__main__":
    main()
