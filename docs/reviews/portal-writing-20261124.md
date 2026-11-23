# Portal and careers interface wording review

Developer and equipment desktop interfaces remain English. Careers remains Korean.
This change updates authored interface text, including the display labels for existing
careers categories; category identifiers, filtering, payloads and URLs are unchanged.

The source review covered 13 static UI/role files and 709 text candidates. The full
keep/change review is `/tmp/cbgpu-copy-backend-review.json`; the applied before/after
manifest is [portal-writing-20261124.json](portal-writing-20261124.json).
Overlapping spelling proposals are covered by their longer replacements.

| Before | After | Reason |
| --- | --- | --- |
| Account security is up to date. | Account security details loaded. | Loading account data does not establish its security posture. |
| This table has no rows. | No rows match the current view. | The current search can exclude existing rows. |
| No audit events have been recorded yet. | No audit events were returned. | Describe the response rather than all historical records. |
| Row updated. | Update request returned. Check the refreshed row to verify the result. | The existing response parser does not validate a successful update acknowledgement. |
| 검토 후 적어주신 이메일로 연락드릴게요. | 지원서 검토와 연락에 적어주신 이메일을 사용해요. | Saving an application does not promise subsequent contact. |
| 내용은 그대로 있으니 잠시 후 다시 시도해주세요. | 입력 내용은 이 화면에 남아 있어요. 안내를 확인한 뒤 다시 제출해 주세요. | Validation errors need correction; preservation only applies to the current page. |

The Korean form also explains that leaving the page loses unsaved input. Onboarding
copy keeps manual invitations, credential guidance and the unsaved-checklist notice.
Consent, privacy/retention terms, technical role names and authored role requirements
are preserved. Terminal text identifies the CBGPU server rather than “this computer”.

## Verification

```sh
npm run check
npm test
npm run build:browser
# Install/use Playwright in a separate tooling package, or resolve it from this package.
CBGPU_BROWSER_TEST_TOOLS=/absolute/path/to/package.json-containing-playwright \
  node developer/tests/portal-writing.browser.mts
# Optional browser executable: CBGPU_BROWSER_EXECUTABLE=/absolute/path/to/chromium
# Reproduce the original copy and capture matching before screenshots:
CBGPU_COPY_BASELINE=3663501 \
CBGPU_BROWSER_TEST_TOOLS=/absolute/path/to/package.json-containing-playwright \
  node developer/tests/portal-writing.browser.mts
```

The browser fixture bundles the real static UI and fulfills every request on loopback.
It opens developer read states and the careers listing, role detail, form, validation
failure, receipt and onboarding clipboard states at 390/1280px in light/dark themes.
It checks retained input and unchanged synthetic payload identifiers. Eight application
requests and four table-update requests are fulfilled by the fixture without any
service, database, login, email or shell execution. All unexpected or external requests
fail the fixture. No credentials or real user records are used.

The malformed successful table acknowledgement is exercised explicitly: the synthetic
row stays unchanged. The original UI said “Row updated”; this change asks the operator
to verify the refreshed row. Rejecting malformed acknowledgements and reconciling write
results remain a separate stability change; this wording change does not implement it.

Before/after screenshots and browser result manifests are written to
`/tmp/cbgpu-backend-writing-before` and `/tmp/cbgpu-backend-writing-after` by default.
Generated browser assets remain build outputs and are not committed. This review does
not apply a database migration, send mail, publish assets or deploy a service.
