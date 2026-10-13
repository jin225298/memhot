#!/usr/bin/env python3
"""Back up live SQLite safely; keep seven private daily archives."""
import gzip
import os
import shutil
import sqlite3
from datetime import datetime, timezone
from pathlib import Path


def main():
    os.umask(0o077)
    data = Path(os.environ.get("MEMHOT_DATA_DIR", "/var/lib/memhot"))
    database = data / "memhot.sqlite"
    if not database.is_file():
        raise SystemExit("Memhot database does not exist; no backup created")
    backups = data / "backups"
    backups.mkdir(mode=0o700, exist_ok=True)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    name = f"memhot-{stamp}.sqlite"
    temporary = backups / (name + ".tmp")
    archive = backups / (name + ".gz")
    compressed = backups / (name + ".gz.tmp")
    try:
        with sqlite3.connect(database.resolve().as_uri() + "?mode=ro", uri=True) as source:
            with sqlite3.connect(temporary) as destination:
                source.backup(destination, pages=256, sleep=0.05)
                if destination.execute("PRAGMA quick_check").fetchone()[0] != "ok":
                    raise RuntimeError("SQLite backup integrity check failed")
        with temporary.open("rb") as source, gzip.open(compressed, "wb", compresslevel=6) as target:
            shutil.copyfileobj(source, target)
        compressed.replace(archive)
        archives = sorted(backups.glob("memhot-*.sqlite.gz"), reverse=True)
        for old in archives[7:]:
            old.unlink()
        print(f"backup={archive.name} bytes={archive.stat().st_size} retained={min(7, len(archives))}")
    finally:
        temporary.unlink(missing_ok=True)
        compressed.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
