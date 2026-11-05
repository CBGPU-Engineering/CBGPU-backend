# Weekday self-study automation

Applications are opt-in, per CBGPU account, and use that account's saved **personal
EduSet student session**. The shared/admin school token is never used for writes.
Students choose a seated room, preferred seat, teacher, reason, periods and a KST
minute between **07:30 and 15:30**. Saturday/Sunday never submit. A time already
passed starts on the next weekday; setting a past time does not immediately apply.

The systemd timer wakes once per minute during the weekday range. The trigger
reads a private host secret from `~/.config/cbgpu/study-auto-secret` and calls the
Next.js machine endpoint. Its secret must also be supplied to the web service as
`CBGPU_STUDY_AUTOMATION_SECRET`, never as a NEXT_PUBLIC variable or repository secret.

## Apply and verify the schema

`schema/20261105_study_automation.sql` creates owner-readable settings/history,
service-only writes and a durable student/date/period submission lease. Apply it
through a separately reviewed operational rollout before the new web release.
The delivery agent intentionally blocks schema changes until an operator has
completed that rollout and acknowledges the exact backend commit.

The scheduler respects the school's official daily policy and opening window.
It waits for official opening until 15:30, skips periods not offered that day,
and never uses another account's connection. Expired/unlinked school accounts
turn automation off and require reconnection and explicit re-enablement.

The preferred seat wins if free. Otherwise squared Euclidean distance in the
same room's current layout determines the nearest free seat; names/IDs break
ties deterministically. Occupancy includes pending applications. If the room is
full, no application is sent; there is no guessed fallback to another room.

## Duplicate and failure behavior

- Existing applications in any status are kept untouched.
- A unique account/date/period run prevents automatic re-execution that day.
- Manual and automatic writes share a durable school-student/date/period lease.
- The claim is marked **submitting before** the external POST. Uncertain results
  remain held even after a restart; reads reconcile but POSTs are not blindly retried.
- A confirmed cancellation releases the relevant submission lease.
- Old interrupted run rows become confirmed-by-readback or uncertain, never a repeat POST.
- Disabling/changing settings stops unclaimed work; an already in-flight request
  may finish. Disabling never cancels an existing school application.

No migration enables automation for existing students. Do not test by submitting
real school applications. Web fixtures cover weekdays, time bounds, proximity,
full rooms, existing applications and lost responses without school HTTP writes.

The app does not need to remain open. Users see results under study → automatic
application settings. Check the host service using:

```bash
systemctl --user status cbgpu-study-auto.timer
journalctl --user -u cbgpu-study-auto.service -n 30
```
