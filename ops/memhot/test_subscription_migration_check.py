"""Fixture-only release verification tests; no service, credentials, or network access."""
import contextlib
import hashlib
import io
import json
from pathlib import Path
import sqlite3
import tempfile
import time
import unittest
from subscription_migration_check import MigrationVerificationError, verify_subscription_migration

SCHEMA = """
CREATE TABLE subscription_sources(id TEXT PRIMARY KEY,scope TEXT NOT NULL,url TEXT NOT NULL,host TEXT NOT NULL,catalog_id TEXT,next_at INTEGER NOT NULL DEFAULT 0,lease_token TEXT,lease_until INTEGER NOT NULL DEFAULT 0,last_attempt INTEGER,last_success INTEGER,failures INTEGER NOT NULL DEFAULT 0,last_new INTEGER NOT NULL DEFAULT 0,error TEXT,UNIQUE(scope,url));
CREATE TABLE subscriptions(id TEXT PRIMARY KEY,user_id TEXT NOT NULL,source_id TEXT NOT NULL,title TEXT NOT NULL,category TEXT NOT NULL,enabled INTEGER NOT NULL DEFAULT 1,origin TEXT NOT NULL DEFAULT 'managed',created INTEGER NOT NULL,UNIQUE(user_id,source_id));
CREATE TABLE subscription_items(source_id TEXT NOT NULL,id TEXT NOT NULL,url TEXT NOT NULL,title TEXT NOT NULL,body TEXT NOT NULL,published TEXT,discovered TEXT NOT NULL,publication_source TEXT NOT NULL,publisher TEXT NOT NULL,first_seen INTEGER NOT NULL,expires_at INTEGER NOT NULL,PRIMARY KEY(source_id,id));
CREATE TABLE subscription_migrations(user_id TEXT PRIMARY KEY);
"""


@contextlib.contextmanager
def database(path):
    conn = sqlite3.connect(path)
    try:
        with conn:
            yield conn
    finally:
        conn.close()


class MigrationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.before = Path(self.temp.name) / 'before.sqlite'
        self.after = Path(self.temp.name) / 'after.sqlite'
        with database(self.before) as conn:
            conn.executescript("""CREATE TABLE users(id TEXT PRIMARY KEY,name TEXT,profile TEXT,epoch INTEGER DEFAULT 0);
                CREATE TABLE records(user_id TEXT,kind TEXT,id TEXT,data TEXT,client_id TEXT,created TEXT,PRIMARY KEY(user_id,kind,id));""")
            profile = {'name': 'A', 'feeds': ['https://example.com/feed?token=SECRET', 'HTTP://EXAMPLE.ORG:80/rss#ignored'],
                       'models': {'text': {'apiKey': 'PRIVATE'}}, 'retentionDays': 30}
            conn.execute('INSERT INTO users VALUES(?,?,?,?)', ('a', 'A', json.dumps(profile), 3))
            conn.execute('INSERT INTO users VALUES(?,?,?,?)', ('b', 'B', json.dumps({'feeds': []}), 0))
            conn.execute('INSERT INTO records VALUES(?,?,?,?,?,?)', ('a', 'note', 'record', '{"private":"BODY"}', None, '2026-10-13'))

    def copy(self):
        with database(self.before) as before, database(self.after) as after:
            before.backup(after)

    def add_source(self, conn, key='source', user='a', url='https://example.com/feed?token=SECRET', host='example.com', origin='legacy'):
        conn.execute('INSERT INTO subscription_sources(id,scope,url,host) VALUES(?,?,?,?)', (key, f'user:{user}', url, host))
        conn.execute('INSERT INTO subscriptions(id,user_id,source_id,title,category,origin,created) VALUES(?,?,?,?,?,?,?)',
                     (key, user, key, host, '其他', origin, int(time.time())))

    def migrate(self):
        self.copy()
        with database(self.after) as conn:
            conn.executescript(SCHEMA)
            for user, raw in conn.execute('SELECT id,profile FROM users').fetchall():
                profile = json.loads(raw)
                profile['feeds'] = []
                conn.execute('UPDATE users SET profile=? WHERE id=?', (json.dumps(profile, sort_keys=True), user))
                conn.execute('INSERT INTO subscription_migrations VALUES(?)', (user,))
            self.add_source(conn)
            self.add_source(conn, 'source2', url='http://example.org/rss', host='example.org')

    def verify(self):
        return verify_subscription_migration(self.before, self.after)

    def reject(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            with self.assertRaises(MigrationVerificationError) as raised:
                self.verify()
        self.assertEqual(output.getvalue(), '')
        for secret in ('SECRET', 'PRIVATE', 'BODY', 'example.com', 'feed?'):
            self.assertNotIn(secret, str(raised.exception))

    def mutate(self, sql, params=()):
        with database(self.after) as conn:
            conn.execute(sql, params)

    def test_exact_migration_normalizes_public_urls_but_preserves_private_ownership(self):
        self.migrate()
        before = [hashlib.sha256(p.read_bytes()).digest() for p in (self.before, self.after)]
        result = self.verify()
        self.assertEqual(result['usersChecked'], 2)
        self.assertEqual(result['legacyFeedReferencesChecked'], 2)
        self.assertEqual(before, [hashlib.sha256(p.read_bytes()).digest() for p in (self.before, self.after)])

    def test_unrelated_profile_edit_rejected(self):
        self.migrate()
        self.mutate("UPDATE users SET profile=json_set(profile,'$.retentionDays',31) WHERE id='a'")
        self.reject()

    def test_json_boolean_is_not_allowed_to_replace_a_number(self):
        self.migrate()
        self.mutate("UPDATE users SET profile=json_set(profile,'$.retentionDays',json('true')) WHERE id='a'")
        self.reject()

    def test_name_epoch_and_identity_changes_rejected(self):
        for sql in ("UPDATE users SET name='Changed' WHERE id='a'", "UPDATE users SET epoch=4 WHERE id='a'", "DELETE FROM users WHERE id='b'"):
            with self.subTest(sql=sql):
                self.after.unlink(missing_ok=True)
                self.migrate()
                self.mutate(sql)
                self.reject()

    def test_record_content_change_rejected(self):
        self.migrate()
        self.mutate("UPDATE records SET data='{}'")
        self.reject()

    def test_missing_or_changed_url_rejected(self):
        for sql in ("DELETE FROM subscriptions WHERE id='source'", "DELETE FROM subscription_sources WHERE id='source'", "UPDATE subscription_sources SET url='https://example.com/different' WHERE id='source'"):
            with self.subTest(sql=sql):
                self.after.unlink(missing_ok=True)
                self.migrate()
                self.mutate(sql)
                self.reject()

    def test_cross_tenant_shared_managed_or_disabled_migration_rejected(self):
        for sql in ("UPDATE subscriptions SET user_id='b' WHERE id='source'", "UPDATE subscription_sources SET scope='user:b' WHERE id='source'", "UPDATE subscription_sources SET catalog_id='catalog' WHERE id='source'", "UPDATE subscriptions SET origin='managed' WHERE id='source'", "UPDATE subscriptions SET enabled=0 WHERE id='source'"):
            with self.subTest(sql=sql):
                self.after.unlink(missing_ok=True)
                self.migrate()
                self.mutate(sql)
                self.reject()

    def test_missing_marker_uncleared_feeds_and_unexpected_source_rejected(self):
        for sql in ("DELETE FROM subscription_migrations WHERE user_id='a'", "UPDATE users SET profile=json_set(profile,'$.feeds',json('[\"https://example.com/feed\"]')) WHERE id='a'", "INSERT INTO subscription_sources(id,scope,url,host) VALUES('extra','user:a','https://extra.example/rss','extra.example')"):
            with self.subTest(sql=sql):
                self.after.unlink(missing_ok=True)
                self.migrate()
                self.mutate(sql)
                self.reject()

    def test_preexisting_tables_rows_and_already_migrated_profiles_preserved(self):
        with database(self.before) as conn:
            conn.executescript(SCHEMA)
            self.add_source(conn, origin='managed')
            self.add_source(conn, 'source2', url='http://example.org/rss', host='example.org')
            conn.execute("INSERT INTO subscription_migrations VALUES('b')")
            conn.execute("INSERT INTO subscription_items VALUES('source','item','https://example.com/article','title','PRIVATE BODY',NULL,'2026-10-13','feed_publication','example.com',1,9999999999)")
        self.copy()
        with database(self.after) as conn:
            conn.execute("UPDATE users SET profile=json_set(profile,'$.feeds',json('[]')) WHERE id='a'")
            conn.execute("INSERT INTO subscription_migrations VALUES('a')")
        self.assertEqual(self.verify()['existingSubscriptionRowsPreserved'], 6)
        for table, sql in (
            ('item', "UPDATE subscription_items SET body='lost'"),
            ('source', "UPDATE subscription_sources SET error='changed' WHERE id='source'"),
            ('relation', "UPDATE subscriptions SET title='changed' WHERE id='source'"),
            ('profile', "UPDATE users SET profile=json_set(profile,'$.feeds',json('[\"https://other.example/rss\"]')) WHERE id='b'")):
            with self.subTest(table=table):
                with database(self.after) as conn:
                    conn.execute('BEGIN')
                    conn.execute(sql)
                    conn.commit()
                self.reject()
                self.after.unlink()
                self.copy()
                with database(self.after) as conn:
                    conn.execute("UPDATE users SET profile=json_set(profile,'$.feeds',json('[]')) WHERE id='a'")
                    conn.execute("INSERT INTO subscription_migrations VALUES('a')")

    def test_default_empty_profiles_still_require_exact_preservation(self):
        with database(self.before) as conn:
            conn.execute("UPDATE users SET profile=json_set(profile,'$.feeds',json('[]'))")
        self.copy()
        with database(self.after) as conn:
            conn.executescript(SCHEMA)
            conn.executemany('INSERT INTO subscription_migrations VALUES(?)', [('a',), ('b',)])
        self.assertEqual(self.verify()['legacyFeedReferencesChecked'], 0)
        self.mutate("UPDATE users SET profile=json_set(profile,'$.models.text.apiKey','changed') WHERE id='a'")
        self.reject()

    def test_known_invalid_local_or_authenticated_feeds_do_not_require_sources(self):
        with database(self.before) as conn:
            conn.execute('UPDATE users SET profile=? WHERE id=?', (json.dumps({'feeds': ['http://127.0.0.1/rss', 'file:///tmp/a', 'https://user:pass@example.com/rss', 'https://test.internal/rss']}), 'a'))
        self.copy()
        with database(self.after) as conn:
            conn.executescript(SCHEMA)
            conn.execute("UPDATE users SET profile=json_set(profile,'$.feeds',json('[]'))")
            conn.executemany('INSERT INTO subscription_migrations VALUES(?)', [('a',), ('b',)])
        self.assertEqual(self.verify()['legacyFeedReferencesChecked'], 0)

    def test_ambiguous_normalization_fails_closed_instead_of_losing_valid_feeds(self):
        for url in ('https://example.com/a/../rss', 'https://例子.公司/rss', 'http://2130706433/rss'):
            with self.subTest(case='ambiguous URL'):
                self.after.unlink(missing_ok=True)
                with database(self.before) as conn:
                    conn.execute("UPDATE users SET profile=? WHERE id='a'", (json.dumps({'feeds': [url]}),))
                self.copy()
                with database(self.after) as conn:
                    conn.executescript(SCHEMA)
                    conn.execute("UPDATE users SET profile=json_set(profile,'$.feeds',json('[]'))")
                    conn.executemany('INSERT INTO subscription_migrations VALUES(?)', [('a',), ('b',)])
                self.reject()


if __name__ == '__main__':
    unittest.main()
