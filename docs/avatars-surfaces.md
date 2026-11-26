# Where avatars apply

Rule (see `avatars.md`): `avatarUrl` is the uploaded photo if there is one, otherwise a default built by the database from the
account's opaque `avatar_seed`. `profileImageUrl` is filled only for the account itself. Across schools **only `avatarUrl`**
crosses the boundary; names, student numbers and schools keep following their existing RLS/RPC rules.

## Source of truth: no replica

The community, DM, member, nomination and GPU-booking builders live in the **same Postgres** as `profiles`
(`cbgpu_schools.*` functions), so nothing is copied. They read `public.profiles.avatar_seed` directly through
`cbgpu_schools.avatar_fields(uuid)`. An event-synced copy would add a second seed that can drift, so it was rejected.
Rust services that talk to the main database use the same database function; services with their own database
(careers, AI, deploy) hold no `profiles` rows to embed.

## Status per surface

Status values: **Applied** (in a migration/PR), **Not applied** (still needed), **N/A** (no user is embedded).

### Database responses

| Surface | Status | How / reason |
| --- | --- | --- |
| `profiles`, canonical card | Applied | computed columns `display_avatar_url`, `profile_image_url`; view `user_cards` → `{id,name,avatarUrl,profileImageUrl}` |
| Community messages (`message_json`) | Applied | migration `20261126150000`: wrapper adds `avatarUrl`, `profileImageUrl`; anonymous `sender_label` unchanged |
| Direct messages (`dm_message_json`) | Applied | same wrapper |
| School members (`member_json`) | Applied | same wrapper |
| Contest operator nominations (`operator_nomination`) | Applied | same wrapper |
| GPU admin bookings (`gpu_admin_booking_item`) | Applied | wrapper reads `owner_id` |
| Study member cards | Applied | `cbgpu_study_member_cards` (visibility from the existing RPC) |
| Study members across schools | Applied | `cbgpu_avatar_urls(uuid[])`: only `{id, avatarUrl}`, signed-in session required, max 500 |
| Clubs API (`cbgpu_school_clubs_api`) | Not applied | large dispatcher; confirm which actions embed a person, then route them through the wrappers |
| `cbgpu_school_api`, `cbgpu_account_api` dispatchers | Not applied | same: embed people through the builders above where they use them, otherwise confirm per action |
| Scoped name-only RPCs (`cbgpu_manufacturing_*`, `cbgpu_contest_operator_profiles`) | Not applied | return `table(id, full_name)`; adding a column changes their contract. Use `cbgpu_avatar_urls` from the client instead |

### Rust services

| Service | Status | Reason / what is needed |
| --- | --- | --- |
| `cbgpu-arcade` (leaderboard, player) | Not applied | rows carry `display_name` from its own table. Needs `avatarUrl` per row via `cbgpu_schools.avatar_fields`-style lookup through a granted definer function for role `cbgpu_arcade_service`, then a new release (reviewed baseline) |
| `cbgpu-contests-api`, `cbgpu-contest-tokens` | N/A | entrant name is typed by the registrant (`attendee_name`), not a profile |
| `cbgpu-contest-teams` | Not applied | copies `attendee_name` into team rows; participant avatars should come from `cbgpu_avatar_urls(user_id)` at read time |
| `cbgpu-schools` (HTTP `/v1/*`) | Applied through the database | its community/DM/member responses are built by the wrapped functions |
| `cbgpu-public-content` | N/A | anonymous public content: intentionally contains no user data |
| `cbgpu-careers` (own database) | N/A | applicants are not `profiles` accounts |
| `cbgpu-ai*`, `cbgpu-accounts`, `cbgpu-gpu-gateway`, `cbgpu-notebook-relay`, `cbgpu-portal-auth` | N/A | no user-facing person objects found in their source |
| `cbgpu-admin-ops` | N/A | returns admin e-mail lists, not displayed people |
| `cbgpu-delivery`, `cbgpu-deploy-control`, `cbgpu-core` | N/A | operational services |

### Web, app, and server-rendered output

| Surface | Status | Reason |
| --- | --- | --- |
| Web: nav, account entry, profile page, legacy study members | Applied (web #77) | |
| Web: study members across schools | Applied (web PR "cross-school avatars") | |
| Web: community / DM message list, contest teams, project team | Not applied | still draw an initial; the data (`avatarUrl`) is now in the community/DM payloads |
| Flutter | Not applied | read `avatarUrl` only; never build provider URLs |
| E-mail, push, OG images | Not applied | none embeds an avatar today. When one does, use `public.cbgpu_default_avatar_url(seed)` or `user_cards` with the service role; never rebuild the URL |

## Guard against silent loss

The wrappers rename the original to `<name>_core`. A later migration that does `create or replace` on the old name would
silently remove avatars. `tests/avatars/migration-guard.test.mts` fails CI if one does; change `<name>_core` instead.
