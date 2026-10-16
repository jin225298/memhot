# ADR-0013: Thoughts belong to explicit calendar days

Status: accepted, 2026-10-16.

## Problem

The active/archived list and night scheduler treated every open historical thought as current. Home synchronization repeatedly returned their full bodies. A source imported late could also appear on the day of processing rather than the conversation's date.

## Decision

A thought keeps one identity and one set of reports. `thought_days` associates `(user_id, thought_id)` with a local calendar date. The initial date comes from the earliest referenced source's `occurredAt` (falling back to source `createdAt`), then the thought creation instant, interpreted in the user's timezone. Existing records are seeded once. Unparseable legacy dates remain in history under 1970-01-01 rather than becoming today's input. Assigned dates do not shift on a later timezone change.

The default client view is Today, with a separate searchable History view. History includes earlier memberships and today's explicitly archived thoughts. A thought carried across multiple days appears once per list result, under its latest matching day; a date filter can still find an earlier association. Carrying never rewrites the original `createdAt` or content.

`POST /api/v1/thoughts/{id}/today` accepts only the current revision. Authentication and the URL identify the record. It atomically adds today's reference, restores an archived thought when needed, and queues only the deterministic daily summary. Repeated requests are idempotent. Carrying does not upload the body, clone the thought, start a model call, or erase its reports. Content edits and model merges never implicitly carry a historical thought.

The client uses `GET /api/v1/thoughts?scope=today|history` and `/api/v1/home?thoughtScope=today`. Legacy unscoped contracts remain available for older clients. Pagination binds scope, filters, timezone, and current date. The client refreshes its day on foregrounding and each minute; historical detail can be fetched independently after returning from a report.

Formation context includes only topics belonging to the source's original local day. Local identity/fingerprint matching still protects against duplicate formation. Durable personal preferences remain available to the model. User-requested discussion or explicit research can still use the selected thought.

Night scheduling and bulk research operate on the relevant day's memberships. The night can finish yesterday's work until the next research hour; it cannot sweep the historical backlog. Execution, model-call and publication checks reject expired automatic work. Content-based research dedup remains in force, so unchanged completed research is retained when a thought is carried. Paid or uncertain receipts must never be erased to authorize replay. Only an explicitly carried, provably unstarted expired task can be rearmed automatically; previously attempted or ambiguous work requires the existing explicit research flow.

## Data and rollout

Membership is tenant-scoped and does not copy source or thought bodies. A records-delete trigger removes memberships for hard-deleted thoughts. Migration is additive and transactional. A production database clone must preserve existing users, records, wallets, billing history, and receipts before activation. Deploy the server before publishing the client update. Rollback uses the stopped-boundary database backup and prior release; post-upgrade writes are preserved for recovery.

## Verification

Backend regressions cover tenant isolation, one-time migration, local dates, stale revision/replay, late formation, deletion cleanup, filtering and pagination, carry without cloning, daily summary references, and preserved research dedup. Frontend tests cover membership-date grouping while retaining original identity/date. An isolated browser fixture checks Today empty state, History, ID/revision-only carry, one resulting card, historical detail restoration, and no horizontal overflow at 390 and 1280 pixels. The fixture uses synthetic records and is removed before packaging.
