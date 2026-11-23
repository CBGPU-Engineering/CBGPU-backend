# School club recruitment campaigns

This feature is school-internal and available to approved schools. It uses the existing native schools API and caller-bound PostgreSQL RPC. No cron, extra service, global publication, annual cleanup, school approval, GPU setup or automatic teacher administrator role is involved.

The additive migration registers globally available `clubs` capability and adds it once to existing APPROVED schools, as explicitly requested. A private backfill record tracks exactly those additions and their configuration revision. LISTED/UNCLAIMED/SUSPENDED schools and all unrelated configuration/IDs/users/roles remain unchanged. Representatives can opt out through existing school settings; rerunning the migration does not re-enable a later opt-out. New application drafts include clubs by default and can uncheck it; approval/identity rules are unchanged.

## Three independent windows

Each arbitrary campaign/round has six absolute timestamps: registration, promotion and application start/end. Each pair must have start < end. Applications must fit inside promotion; registration end must be no later than promotion end. Registration can overlap promotion. Multiple campaigns in the same year are valid. Every mutation evaluates the actual server clock after acquiring the relevant row lock, without a cron security dependency. The RPC bounds row-lock waits to two seconds.

Verified school STUDENT members can prepare private DRAFT club posts at any time. Only a fresh registration window permits READY submission or READY edits. Founder submission is explicit and does not require a new central approval. READY posts automatically appear only during ACTIVE promotion. School members can select a past campaign to read its READY history; future posts remain private. PAUSED campaigns block new readiness/publication/application activity. Draft preparation, existing application review and withdrawal retain their separate permissions.

Applying requires the exact school, an active verified STUDENT membership, published READY post and current ACTIVE application window. One application row per person/club/campaign is preserved across review and withdrawal. Club-owner or school-administrator review never grants access to other schools or other founders' private applicants. Every write uses caller identity, never JSON owner/user/role claims. Global account_type EXTERNAL does not override a verified school STUDENT membership. Plain TEACHER membership is not an administrator role.

## API

Native prefix `/v1/schools/:school/clubs`; existing web schools proxy prefix `/api/schools/schools/:school/clubs`.

- GET base: optional campaign_id UUID, limit1..100/default50, offset0..100000. Returns school_id/server_now/can_manage_campaigns/campaigns/items/total. Default feed includes currently promoted posts and managed drafts. A selected past campaign includes historical READY posts. School representatives receive actual per-campaign club_count/application_count; ordinary members receive neither. No applicant PII or other applicants' answers are in the feed.
- POST campaigns: confirmed:true, title and six timestamps. PATCH campaigns/:campaign: confirmed:true, expected_revision and partial fields/title/status ACTIVE|PAUSED. School owner/admin only.
- POST base: campaign_id,title,description,application_prompt,capacity (null or1..10000). Creates DRAFT with authenticated founder.
- PATCH :club: expected_revision and partial title/description/application_prompt/capacity. POST :club/ready or :club/withdraw: expected_revision.
- POST :club/applications: answers (up to10000 characters). Duplicate attempts fail409, never create a second row.
- GET :club/applications: manager only, paged items/total. Applicant identity includes bound user_id, bounded full_name (empty allowed), exact school membership student_number (nullable). Applicant answers remain private.
- PATCH :club/applications/:application: expected_revision,status APPROVED|REJECTED. Approval locks the club row before counting approved applications, so concurrent approvals cannot exceed capacity. POST the same path/withdraw: expected_revision; applicant only.

Campaigns expose registration_open/promotion_open/application_open/next_at from the server clock. Clubs expose published/can_manage/can_edit/can_submit_ready/can_withdraw/can_apply and own my_application. Frontend clocks are informational, not authorization. All IDs are school-bound by joins/composite foreign keys; JSON cannot override the path/actor. Writes do not retry automatically after ambiguous responses; refresh and compare revisions before another explicit action.

## Production boundary and rollback

Prepare a private full backup and original validate_draft definition, feature registry row/inventory and native schools executable before applying **only** this migration. No live data/account/job/container/controller action is part of source tests. New private tables have RLS enabled and no direct browser/service-role grants. The sole browser RPC is EXECUTE-only for authenticated callers and still validates the actual auth.sessions/account/school/membership on each call.

The runtime change, after review, is only the existing schools HTTP executable. Preserve its environment/account flags and listening address. It does not start another worker or touch GPU/SSH jobs. Only the tracked APPROVED-school feature addition is automatic. No memberships, approvals or roles are granted; unregistered catalog institutions stay inactive.

`rollback.sql` disables the new RPC/capability and removes only tracked feature additions whose exact features/revision still match the migration. It preserves later representative choices and all campaign/club/application/audit rows and draft compatibility. Do not drop tables, clear schools' feature arrays or delete applicant history. Restoring the prior native schools executable removes the new routes. Re-enabling after rollback requires a separate explicit operator decision; rerunning the migration does not silently enable the registry row.

## Verification

The disposable network-none PostgreSQL fixture applies twice, tests multiple same-year rounds, overlapping windows, fresh school STUDENT/TEACHER/role distinctions, private drafts, exact tenant boundaries, applicant privacy, revision conflict, pause/closure/future/history behavior and revoked memberships/sessions. Two independent SQL sessions race to approve a capacity-one club: exactly one commits and one returns CLUB_CAPACITY_FULL. HTTP fixtures exercise every new route, caller JWT forwarding, bound path IDs, spoofed owner/status/school rejection and duplicate/invalid queries. All institutions/accounts/data are synthetic; no production identity is used.
