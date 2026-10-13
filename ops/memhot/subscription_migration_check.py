"""Read-only, fail-closed verification of the one-time profile.feeds migration.

Never log rows, profiles, feed URLs, credentials, or identifiers. URL normalization
intentionally covers ordinary HTTP(S) feed URLs; ambiguous WHATWG forms require
operator review instead of granting a broader preservation exception.
"""
from contextlib import closing
from collections import defaultdict
import hashlib
import ipaddress
import json
from pathlib import Path
import sqlite3
import time
from urllib.parse import quote, urlsplit, urlunsplit


class MigrationVerificationError(RuntimeError):
    """Safe-to-display explanation containing no database content."""


TABLE_KEYS = {
    'subscription_sources': ('id',),
    'subscriptions': ('id',),
    'subscription_items': ('source_id', 'id'),
    'subscription_migrations': ('user_id',),
}


def _require(condition, message):
    if not condition:
        raise MigrationVerificationError(message)


def _json(raw):
    def invalid(_):
        raise ValueError('non-JSON constant')
    return json.loads(raw, parse_constant=invalid)


def _canonical_json(value):
    # Unlike Python equality, this distinguishes true, 1, and 1.0.
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':'), allow_nan=False)


def _columns(conn, table):
    return tuple(row[1] for row in conn.execute(f'PRAGMA table_info("{table}")'))


def _digest(conn, table, columns, order):
    hasher = hashlib.sha256()
    names = ','.join(f'"{name}"' for name in columns)
    ordered = ','.join(f'"{name}"' for name in order)
    for row in conn.execute(f'SELECT {names} FROM "{table}" ORDER BY {ordered}'):
        encoded = _canonical_json(tuple(row)).encode()
        hasher.update(len(encoded).to_bytes(8, 'big'))
        hasher.update(encoded)
    return hasher.digest()


def _blocked(address):
    # Same address ranges as network::blocked; no DNS or external requests here.
    if address.version == 6:
        if address.ipv4_mapped:
            return _blocked(address.ipv4_mapped)
        first, second = int(address) >> 112, (int(address) >> 96) & 0xffff
        return not 0x2000 <= first <= 0x3fff or first == 0x2002 or (
            first == 0x2001 and (second == 0 or second == 0xdb8 or second < 0x200))
    a, b, c, _ = address.packed
    return (a in (0, 10, 127) or a >= 224 or (a == 100 and 64 <= b <= 127)
            or (a == 169 and b == 254) or (a == 172 and 16 <= b <= 31)
            or (a == 192 and (b == 168 or (b == 0 and c in (0, 2)) or (b == 88 and c == 99)))
            or (a == 198 and (b in (18, 19) or (b == 51 and c == 100)))
            or (a == 203 and b == 0 and c == 113))


def _canonical_feed(raw):
    """Return (canonical URL, display host), or None for demonstrably rejected input."""
    raw = raw.strip()
    try:
        parts = urlsplit(raw)
        scheme, host, port = parts.scheme.lower(), parts.hostname, parts.port
        if scheme not in ('http', 'https'):
            return None
        if parts.username not in (None, '') or parts.password is not None:
            return None
        _require(parts.username is None, 'legacy feed URL requires manual normalization review')
    except ValueError:
        raise MigrationVerificationError('legacy feed URL requires manual normalization review') from None
    _require(host is not None, 'legacy feed URL requires manual normalization review')
    _require(all(32 <= ord(c) < 127 for c in raw) and '\\' not in raw,
             'legacy feed URL requires manual normalization review')
    host = host.lower()
    if host == 'localhost' or host.endswith(('.localhost', '.local', '.internal')):
        return None
    _require('%' not in host, 'legacy feed URL requires manual normalization review')
    try:
        address = ipaddress.ip_address(host)
    except ValueError:
        address = None
        # WHATWG also recognizes short, octal, hex, and integer IPv4 spellings.
        _require(not host.rstrip('.').split('.')[-1].isdigit()
                 and not host.lower().split('.')[-1].startswith('0x'),
                 'legacy feed URL requires manual normalization review')
    if address:
        if _blocked(address):
            return None
        host = f'[{address.compressed}]' if address.version == 6 else address.compressed
    path = parts.path or '/'
    _require(not any(segment.lower().replace('%2e', '.') in ('.', '..') for segment in path.split('/')),
             'legacy feed URL requires manual normalization review')
    authority = host + (f':{port}' if port is not None and port != (443 if scheme == 'https' else 80) else '')
    path = quote(path, safe="/!$&'()*+,-.:;=@_~%[]")
    query = quote(parts.query, safe="/!$&'()*+,-.:;=?@_~%[]")
    result = urlunsplit((scheme, authority, path, query, ''))
    if '?' in raw.split('#', 1)[0] and not parts.query:
        result += '?'
    return result, host


def _rows(conn, table, keys):
    columns = _columns(conn, table)
    if not columns:
        return {}, columns
    rows = {}
    for row in conn.execute(f'SELECT * FROM "{table}"'):
        value = dict(row)
        key = tuple(value[column] for column in keys)
        _require(key not in rows, 'duplicate identity in migration evidence')
        rows[key] = value
    return rows, columns


def _preserve_existing(before, after, table, keys):
    old, columns = _rows(before, table, keys)
    new, new_columns = _rows(after, table, keys)
    _require(bool(new_columns), 'subscription migration schema is incomplete')
    _require(set(columns).issubset(new_columns), 'existing subscription columns were removed')
    for key, value in old.items():
        _require(key in new, 'existing subscription data was removed')
        _require(_canonical_json(value) == _canonical_json({c: new[key][c] for c in columns}),
                 'existing subscription data was modified')
    return old, new


def _verify(before, after):
    old_users, user_columns = _rows(before, 'users', ('id',))
    new_users, new_columns = _rows(after, 'users', ('id',))
    _require(set(old_users) == set(new_users), 'migration changed user identities')
    _require(set(user_columns).issubset(new_columns), 'migration removed user columns')
    record_columns = _columns(before, 'records')
    _require(record_columns and record_columns == _columns(after, 'records'), 'migration changed record schema')
    _require(_digest(before, 'records', record_columns, ('user_id', 'kind', 'id'))
             == _digest(after, 'records', record_columns, ('user_id', 'kind', 'id')),
             'migration changed existing records')
    tables = {name: _preserve_existing(before, after, name, keys)
              for name, keys in TABLE_KEYS.items() if name != 'subscription_items'}
    old_sources, new_sources = tables['subscription_sources']
    old_subs, new_subs = tables['subscriptions']
    old_marks, new_marks = tables['subscription_migrations']
    # Collected bodies can be large; compare their ordered digest without retaining them in memory.
    item_columns = _columns(before, 'subscription_items')
    new_item_columns = _columns(after, 'subscription_items')
    _require(bool(new_item_columns), 'subscription migration schema is incomplete')
    old_item_count = before.execute('SELECT count(*) FROM subscription_items').fetchone()[0] if item_columns else 0
    _require(after.execute('SELECT count(*) FROM subscription_items').fetchone()[0] == old_item_count,
             'migration unexpectedly changed collected items')
    if item_columns:
        _require(item_columns == new_item_columns
                 and _digest(before, 'subscription_items', item_columns, ('source_id', 'id'))
                 == _digest(after, 'subscription_items', item_columns, ('source_id', 'id')),
                 'existing collected items were modified')
    _require(set(new_marks) == set(old_users), 'migration markers do not match existing users')
    allowed_sources, allowed_subs = set(old_sources), set(old_subs)
    sources_by_url, subscriptions_by_owner = defaultdict(list), defaultdict(list)
    for source in new_sources.values():
        sources_by_url[(source['scope'], source['url'])].append(source)
    for sub in new_subs.values():
        subscriptions_by_owner[(sub['user_id'], sub['source_id'])].append(sub)
    migrated = 0
    for key, old in old_users.items():
        new = new_users[key]
        for column in user_columns:
            if column != 'profile':
                _require(old[column] == new[column], 'migration changed existing user attributes')
        original, current = _json(old['profile']), _json(new['profile'])
        _require(isinstance(original, dict) and isinstance(current, dict), 'user profile is not a JSON object')
        if key in old_marks:
            _require(_canonical_json(original) == _canonical_json(current), 'migration changed an already migrated profile')
            continue
        expected = dict(original)
        expected['feeds'] = []
        _require(_canonical_json(current) == _canonical_json(expected), 'migration changed profile fields outside feeds')
        raw_feeds = original.get('feeds')
        raw_feeds = raw_feeds if isinstance(raw_feeds, list) else []
        feeds = [feed for feed in raw_feeds if isinstance(feed, str)]
        _require(len(feeds) <= 20, 'legacy feed count requires manual migration review')
        for feed in feeds:
            parsed = _canonical_feed(feed)
            if parsed is None:
                continue
            url, host = parsed
            user_id = key[0]
            candidates = sources_by_url[(f'user:{user_id}', url)]
            _require(len(candidates) == 1, 'legacy feed private source or URL was not preserved')
            source = candidates[0]
            _require(source['catalog_id'] is None and source['host'] == host,
                     'legacy feed private scope was not preserved')
            source_key = (source['id'],)
            relations = subscriptions_by_owner[(user_id, source['id'])]
            _require(len(relations) == 1, 'legacy feed ownership was not preserved')
            relation = relations[0]
            relation_key = (relation['id'],)
            if source_key not in old_sources:
                defaults = {'next_at': 0, 'lease_token': None, 'lease_until': 0, 'last_attempt': None,
                            'last_success': None, 'failures': 0, 'last_new': 0, 'error': None}
                _require(all(source.get(k) == v for k, v in defaults.items()),
                         'migration unexpectedly fetched or changed a new source')
            if relation_key not in old_subs:
                _require(relation['origin'] == 'legacy' and relation['enabled'] == 1
                         and relation['title'] == host and relation['category'] == '其他'
                         and isinstance(relation['created'], int) and 0 <= relation['created'] <= time.time() + 60,
                         'legacy subscription defaults were not preserved')
            allowed_sources.add(source_key)
            allowed_subs.add(relation_key)
            migrated += 1
    _require(set(new_sources) == allowed_sources, 'migration added unexpected subscription sources')
    _require(set(new_subs) == allowed_subs, 'migration added unexpected subscription relations')
    return {'usersChecked': len(old_users), 'legacyFeedReferencesChecked': migrated,
            'existingSubscriptionRowsPreserved': sum(len(old) for old, _ in tables.values()) + old_item_count}


def verify_subscription_migration(before_path, after_path):
    """Accept only the expected migration, raising a content-free error otherwise."""
    try:
        with closing(sqlite3.connect(Path(before_path).resolve().as_uri() + '?mode=ro', uri=True)) as before:
            with closing(sqlite3.connect(Path(after_path).resolve().as_uri() + '?mode=ro', uri=True)) as after:
                before.row_factory = after.row_factory = sqlite3.Row
                _require(before.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'
                         and after.execute('PRAGMA integrity_check').fetchone()[0] == 'ok',
                         'migration evidence failed database integrity')
                return _verify(before, after)
    except MigrationVerificationError:
        raise
    except (OSError, ValueError, KeyError, TypeError, sqlite3.Error):
        raise MigrationVerificationError('subscription migration verification could not read valid evidence') from None
