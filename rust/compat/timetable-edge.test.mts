import test from "node:test";
import assert from "node:assert/strict";
import {
  createTimetableProxy,
  centralTimetableOrigin,
} from "../../volumes/functions/timetable/proxy.ts";
const uid = "00000000-0000-4000-8000-000000000001";
function fixture(multi = false) {
  let authStatus = 200,
    oracle: unknown = { valid: true, approved: true, school_id: "cbsh" },
    selected = "school-b",
    membershipValid = true,
    centralStatus = 200,
    centralSchool: string | null = null;
  const calls: Array<{ url: string; headers: Headers }> = [];
  const config = {
    multiSchool: multi,
    supabaseUrl: "http://envoy:8000",
    anonKey: "synthetic-anon",
    centralOrigin: "https://cbgpu.com",
  };
  const handler = createTimetableProxy(config, async (input, options) => {
    const url = String(input),
      headers = new Headers(options?.headers);
    calls.push({ url, headers });
    if (url.endsWith("/auth/v1/user"))
      return Response.json(
        { id: uid, email_confirmed_at: "2026-01-01T00:00:00Z" },
        { status: authStatus },
      );
    if (url.endsWith("/rpc/cbgpu_self_session_status"))
      return Response.json(oracle ?? {}, {
        status: oracle === null ? 404 : 200,
      });
    if (url.endsWith("/api/schools/memberships"))
      return Response.json({
        current_school_id: selected,
        items: [
          {
            user_id: uid,
            school_id: selected,
            status: membershipValid ? "ACTIVE" : "SUSPENDED",
            verified: true,
            school: {
              id: selected,
              status: "APPROVED",
              official_name: "선택 학교",
            },
          },
        ],
      });
    if (url.includes("/api/timetable?"))
      return Response.json(
        {
          grid: {
            0: {
              1: {
                subject: "수학",
                teacher: "비공개 교사",
                changed: false,
                group: "A",
                email: "must-not-leak",
              },
            },
          },
          times: ["09:00"],
          grades: [1, 2, 3],
          classes: [1, 2],
          school: centralSchool ?? (multi ? "선택 학교" : "충북과학고등학교"),
          ...(multi ? { school_id: selected } : {}),
          student_records: ["must-not-leak"],
          stale: false,
          weekStart: "2026-11-16",
        },
        { status: centralStatus },
      );
    throw Error("Unexpected upstream");
  });
  return {
    handler,
    calls,
    deny: () => (authStatus = 401),
    missingOracle: () => (oracle = null),
    revoke: () => (oracle = { valid: false, approved: false, school_id: null }),
    external: () =>
      (oracle = { valid: true, approved: true, school_id: "another" }),
    suspend: () => (membershipValid = false),
    wrongCentral: () => (centralSchool = "wrong school"),
    centralFailure: () => (centralStatus = 503),
  };
}
function request(query = "grade=1&class=1", token?: string, origin?: string) {
  return new Request(
    "https://cbgpu.com/backend/functions/v1/timetable?" + query,
    {
      headers: {
        ...(token ? { Authorization: "Bearer " + token } : {}),
        ...(origin ? { Origin: origin } : {}),
      },
    },
  );
}
test("released unsigned legacy widgets receive only public CBsh subjects with original DTO keys", async () => {
  const f = fixture();
  const r = await f.handler(request());
  assert.equal(r.status, 200);
  const body = await r.json();
  assert.equal(body.school, "충북과학고등학교");
  assert.equal(body.grid[0][1].subject, "수학");
  assert.equal(body.grid[0][1].teacher, "");
  for (const key of ["grid", "times", "grades", "classes", "school"])
    assert.ok(key in body);
  assert.equal(body.student_records, undefined);
  assert.equal(body.grid[0][1].email, undefined);
  assert.equal(f.calls.length, 1);
  assert.equal(f.calls[0].headers.has("authorization"), false);
  assert.equal(r.headers.get("cache-control"), "no-store");
  assert.equal(r.headers.get("access-control-allow-origin"), null);
});
test("authenticated full fields require fresh GoTrue AND self-session oracle, never cached or downgraded", async () => {
  const f = fixture();
  let r = await f.handler(request(undefined, "user-jwt"));
  assert.equal(r.status, 200);
  assert.equal((await r.json()).grid[0][1].teacher, "비공개 교사");
  assert.equal(f.calls.length, 3);
  assert.ok(
    f.calls.every((c) => c.headers.get("authorization") === "Bearer user-jwt"),
  );
  f.revoke();
  r = await f.handler(request(undefined, "user-jwt"));
  assert.equal(r.status, 401);
  assert.equal(
    f.calls.filter((c) => c.url.includes("/api/timetable?")).length,
    1,
  );
  const missing = fixture();
  missing.missingOracle();
  r = await missing.handler(request(undefined, "user-jwt"));
  assert.equal(r.status, 403);
  assert.equal((await r.json()).code, "SESSION_VERIFICATION_UNAVAILABLE");
  assert.equal(missing.calls.length, 2);
  const denied = fixture();
  denied.deny();
  assert.equal((await denied.handler(request(undefined, "bad"))).status, 401);
  assert.equal(denied.calls.length, 1);
  const other = fixture();
  other.external();
  assert.equal(
    (await other.handler(request(undefined, "valid-other-school"))).status,
    403,
  );
});
test("V1 derives server-selected verified school and cannot fall back to CBsh or a client-supplied school", async () => {
  const f = fixture(true);
  assert.equal((await f.handler(request())).status, 401);
  assert.equal(f.calls.length, 0);
  let r = await f.handler(request("grade=1&class=25", "user-jwt"));
  assert.equal(r.status, 200);
  const central = f.calls.find((c) => c.url.includes("/api/timetable?"))!;
  assert.equal(new URL(central.url).searchParams.get("school_id"), "school-b");
  assert.equal(central.headers.get("authorization"), "Bearer user-jwt");
  assert.equal((await r.json()).school_id, "school-b");
  r = await f.handler(request("school_id=cbsh", "user-jwt"));
  assert.equal(r.status, 409);
  const before = f.calls.length;
  const wrong = fixture(true);
  wrong.wrongCentral();
  assert.equal(
    (await wrong.handler(request(undefined, "user-jwt"))).status,
    503,
  );
  f.suspend();
  assert.equal((await f.handler(request(undefined, "user-jwt"))).status, 403);
  assert.equal(f.calls.length, before + 2);
});
test("CORS, central origins, service credentials and request bounds fail closed", async () => {
  const f = fixture();
  assert.equal(
    (
      await f.handler(
        request(undefined, undefined, "https://evil.front.cbgpu.com"),
      )
    ).status,
    403,
  );
  let r = await f.handler(
    request(undefined, undefined, "https://app.cbgpu.com"),
  );
  assert.equal(
    r.headers.get("access-control-allow-origin"),
    "https://app.cbgpu.com",
  );
  const privileged =
    "x." +
    Buffer.from(JSON.stringify({ role: "service_role" })).toString(
      "base64url",
    ) +
    ".x";
  const count = f.calls.length;
  assert.equal((await f.handler(request(undefined, privileged))).status, 401);
  assert.equal(f.calls.length, count);
  assert.equal((await f.handler(request("grade=1evil"))).status, 400);
  assert.equal(
    (await f.handler(request("url=http://169.254.169.254"))).status,
    400,
  );
  assert.equal((await f.handler(request("school_id=other"))).status, 409);
  for (const centralOrigin of [
    "https://evil.front.cbgpu.com",
    "http://127.0.0.1:3000",
    "https://cbgpu.com.evil.invalid",
    "https://u:p@cbgpu.com",
    "http://169.254.169.254",
  ])
    assert.throws(() =>
      centralTimetableOrigin({
        centralOrigin,
        multiSchool: false,
        supabaseUrl: "http://envoy",
        anonKey: "fake",
      }),
    );
  assert.equal(
    centralTimetableOrigin({
      centralOrigin: "http://web:3000",
      privateContainer: true,
      multiSchool: false,
      supabaseUrl: "http://envoy",
      anonKey: "fake",
    }).host,
    "web:3000",
  );
  r = await f.handler(
    new Request("https://cbgpu.com/backend/functions/v1/timetable", {
      method: "POST",
    }),
  );
  assert.equal(r.status, 405);
});
test("upstream failure returns no stale private teacher cache", async () => {
  const f = fixture();
  assert.equal((await f.handler(request(undefined, "user-jwt"))).status, 200);
  f.centralFailure();
  const r = await f.handler(request(undefined, "user-jwt"));
  assert.equal(r.status, 503);
  assert.equal((await r.json()).grid, undefined);
});

test("response limits and project-anon compatibility cannot expose privileged fields", async () => {
  const config = {
    multiSchool: false,
    supabaseUrl: "http://envoy:8000",
    anonKey: "synthetic-anon",
  };
  const large = createTimetableProxy(
    config,
    async () => new Response("x".repeat(300000)),
  );
  assert.equal((await large(request())).status, 503);
  const anon =
    "x." +
    Buffer.from(JSON.stringify({ role: "anon" })).toString("base64url") +
    ".x";
  const f = fixture();
  let r = await f.handler(request(undefined, anon));
  assert.equal(r.status, 200);
  assert.equal((await r.json()).grid[0][1].teacher, "");
  assert.equal(f.calls.length, 1);
  const scoped = fixture(true);
  assert.equal((await scoped.handler(request(undefined, anon))).status, 401);
  assert.equal(scoped.calls.length, 0);
  r = await f.handler(request("x".repeat(3000)));
  assert.equal(r.status, 414);
});
