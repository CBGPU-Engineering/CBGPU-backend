/** Compatibility transport only. The central website owns the sole timetable decoder/cache. */
export type TimetableProxyConfig = {
  centralOrigin?: string;
  privateContainer?: boolean;
  supabaseUrl: string;
  anonKey: string;
  multiSchool: boolean;
};
const ORIGINS = new Set([
  "https://cbgpu.com",
  "https://www.cbgpu.com",
  "https://app.cbgpu.com",
  "https://student.cbgpu.com",
  "https://researcher.cbgpu.com",
  "https://developer.cbgpu.com",
  "https://admin.cbgpu.com",
  "https://developers.cbgpu.com",
  "https://print.cbgpu.com",
  "https://laser.cbgpu.com",
  "https://manufacturing.cbgpu.com",
]);
const PRIVATE_HOSTS = new Set([
  "127.0.0.1",
  "localhost",
  "[::1]",
  "web",
  "frontend",
  "cbgpu-web",
  "host.docker.internal",
]);
const TRUSTED_CENTRAL = new Set([
  "cbgpu.com",
  "www.cbgpu.com",
  "app.cbgpu.com",
]);
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
export function centralTimetableOrigin(config: TimetableProxyConfig): URL {
  const url = new URL(config.centralOrigin || "https://cbgpu.com");
  if (
    url.username ||
    url.password ||
    url.search ||
    url.hash ||
    url.pathname !== "/"
  )
    throw Error("Untrusted central origin");
  if (
    url.protocol === "https:" &&
    TRUSTED_CENTRAL.has(url.hostname) &&
    (!url.port || url.port === "443")
  )
    return url;
  if (
    config.privateContainer &&
    url.protocol === "http:" &&
    PRIVATE_HOSTS.has(url.hostname)
  )
    return url;
  throw Error("Untrusted central origin");
}
async function readJson(
  response: Response,
  limit = 262144,
): Promise<Record<string, unknown>> {
  if (Number(response.headers.get("content-length") || 0) > limit)
    throw Error("Upstream response exceeds limit");
  if (!response.body) throw Error("Empty upstream response");
  const reader = response.body.getReader();
  let size = 0;
  const chunks: Uint8Array[] = [];
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > limit) {
        await reader.cancel();
        throw Error("Upstream response exceeds limit");
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }
  const body = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  const value = JSON.parse(new TextDecoder().decode(body));
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw Error("Invalid upstream JSON");
  return value;
}
function record(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === "object" && !Array.isArray(v);
}
function boundedText(v: unknown, max: number): v is string {
  return (
    typeof v === "string" &&
    v.length <= max &&
    !/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/.test(v)
  );
}
export function widgetTimetableDto(
  input: Record<string, unknown>,
  expectedSchool: string,
  publicOnly: boolean,
  expectedName?: string,
) {
  if (
    !boundedText(input.school, 120) ||
    !input.school ||
    (expectedName !== undefined && input.school !== expectedName) ||
    (expectedSchool === "cbsh" && input.school !== "충북과학고등학교") ||
    (input.school_id !== undefined && input.school_id !== expectedSchool) ||
    !record(input.grid)
  )
    throw Error("Timetable school mismatch");
  const grid: Record<string, Record<string, unknown>> = {};
  for (const [day, cells] of Object.entries(input.grid)) {
    if (!/^[0-4]$/.test(day) || !record(cells))
      throw Error("Invalid timetable grid");
    const selected: Record<string, unknown> = {};
    for (const [period, value] of Object.entries(cells)) {
      if (
        !/^(?:[1-9]|1[0-2])$/.test(period) ||
        !record(value) ||
        !boundedText(value.subject, 256) ||
        !boundedText(value.teacher ?? "", 128) ||
        !boundedText(value.group ?? "", 8)
      )
        throw Error("Invalid timetable cell");
      selected[period] = {
        subject: value.subject,
        teacher: publicOnly ? "" : (value.teacher ?? ""),
        changed: value.changed === true,
        group: value.group ?? "",
      };
    }
    grid[day] = selected;
  }
  const grades = input.grades,
    classes = input.classes,
    times = input.times;
  if (
    !Array.isArray(grades) ||
    grades.length > 12 ||
    !grades.every((n) => Number.isInteger(n) && n >= 1 && n <= 12) ||
    !Array.isArray(classes) ||
    classes.length > 100 ||
    !classes.every((n) => Number.isInteger(n) && n >= 1 && n <= 100) ||
    !Array.isArray(times) ||
    times.length > 24 ||
    !times.every((t) => boundedText(t, 80))
  )
    throw Error("Invalid timetable choices");
  return {
    grid,
    times,
    grades,
    classes,
    school: input.school,
    school_id: expectedSchool,
    scope: publicOnly ? "legacy-public-subjects" : "verified-school",
    ...(typeof input.stale === "boolean" ? { stale: input.stale } : {}),
    ...(boundedText(input.fetchedAt, 40) ? { fetchedAt: input.fetchedAt } : {}),
    ...(typeof input.weekStart === "string" &&
    /^\d{4}-\d{2}-\d{2}$/.test(input.weekStart)
      ? { weekStart: input.weekStart }
      : {}),
  };
}
// Unverified claims can only reject credentials or select the already-public
// legacy subject-only path. They NEVER authorize user identity or teacher fields.
function claimedAnonymousToken(token: string) {
  try {
    const part = token.split(".")[1];
    if (!part || part.length > 8192) return false;
    return (
      JSON.parse(atob(part.replace(/-/g, "+").replace(/_/g, "/"))).role ===
      "anon"
    );
  } catch {
    return false;
  }
}
function claimedPrivilegedToken(token: string) {
  if (token.startsWith("sb_secret_")) return true;
  try {
    const part = token.split(".")[1];
    if (!part || part.length > 8192) return false;
    const value = JSON.parse(atob(part.replace(/-/g, "+").replace(/_/g, "/")));
    return value.role === "service_role" || value.role === "supabase_admin";
  } catch {
    return false;
  }
}
export function createTimetableProxy(
  config: TimetableProxyConfig,
  fetcher: typeof fetch = fetch,
) {
  return async (req: Request): Promise<Response> => {
    const deadline = Date.now() + 18_000;
    const origin = req.headers.get("origin"),
      cors: Record<string, string> = {
        "Cache-Control": "no-store",
        Vary: "Origin, Authorization",
        "X-Content-Type-Options": "nosniff",
        "Access-Control-Allow-Methods": "GET, OPTIONS",
        "Access-Control-Allow-Headers":
          "Authorization, Content-Type, apikey, x-cbgpu-auth",
        "Access-Control-Max-Age": "300",
      };
    const reply = (body: unknown, status = 200) =>
      Response.json(body, { status, headers: cors });
    if (origin && !ORIGINS.has(origin))
      return reply(
        { error: "허용되지 않은 요청 출처입니다.", code: "ORIGIN_DENIED" },
        403,
      );
    if (origin) cors["Access-Control-Allow-Origin"] = origin;
    if (req.method === "OPTIONS")
      return new Response(null, { status: 204, headers: cors });
    if (req.method !== "GET")
      return reply(
        { error: "GET 요청만 지원합니다.", code: "METHOD_NOT_ALLOWED" },
        405,
      );
    if (Number(req.headers.get("content-length") || 0) > 0)
      return reply(
        { error: "GET 본문은 지원하지 않습니다.", code: "INVALID_REQUEST" },
        400,
      );
    if (req.url.length > 2048)
      return reply(
        { error: "시간표 요청이 너무 길어요.", code: "INVALID_REQUEST" },
        414,
      );
    const query = new URL(req.url).searchParams;
    if (
      [...query.keys()].some(
        (key) => !["grade", "class", "refresh", "school_id"].includes(key),
      )
    )
      return reply(
        { error: "시간표 요청을 확인해주세요.", code: "INVALID_REQUEST" },
        400,
      );
    const grade = query.get("grade") || "1",
      cls = query.get("class") || "1";
    if (
      !/^[1-9]\d{0,2}$/.test(grade) ||
      Number(grade) > (config.multiSchool ? 12 : 3) ||
      !/^[1-9]\d{0,2}$/.test(cls) ||
      Number(cls) > (config.multiSchool ? 100 : 20) ||
      (query.has("refresh") && !["0", "1"].includes(query.get("refresh")!))
    )
      return reply(
        { error: "학년과 반을 확인해주세요.", code: "INVALID_CLASS" },
        400,
      );
    let central: URL, authBase: URL;
    try {
      central = centralTimetableOrigin(config);
      authBase = new URL(config.supabaseUrl);
      if (
        !["http:", "https:"].includes(authBase.protocol) ||
        authBase.username ||
        authBase.password ||
        authBase.search ||
        authBase.hash ||
        !config.anonKey ||
        claimedPrivilegedToken(config.anonKey)
      )
        throw Error();
    } catch {
      return reply(
        {
          error: "시간표 서비스 연결을 준비하고 있어요.",
          code: "CONFIGURATION_UNAVAILABLE",
        },
        503,
      );
    }
    const authorization = req.headers.get("authorization"),
      alternate = req.headers.get("x-cbgpu-auth");
    if (
      alternate &&
      authorization &&
      authorization !== "Bearer " + config.anonKey &&
      authorization !== alternate
    )
      return reply(
        { error: "로그인 요청을 확인해주세요.", code: "AMBIGUOUS_AUTH" },
        400,
      );
    const supplied = alternate || authorization;
    let token: string | null = null;
    if (supplied) {
      const match = supplied.match(/^Bearer ([^\s,]{1,8192})$/i);
      if (!match)
        return reply(
          { error: "로그인 후 이용해주세요.", code: "INVALID_SESSION" },
          401,
        );
      if (match[1] !== config.anonKey && !claimedAnonymousToken(match[1]))
        token = match[1];
    }
    if (token && claimedPrivilegedToken(token))
      return reply(
        { error: "사용자 로그인이 필요합니다.", code: "USER_SESSION_REQUIRED" },
        401,
      );
    if (config.multiSchool && !token)
      return reply(
        {
          error: "학교 시간표를 보려면 앱 업데이트 후 로그인해주세요.",
          code: "SCOPED_WIDGET_LOGIN_REQUIRED",
        },
        401,
      );
    let schoolId = "cbsh",
      schoolName = "충북과학고등학교";
    const publicOnly = !token;
    const upstream = async (url: URL, init: RequestInit = {}) => {
      const remaining = deadline - Date.now();
      if (remaining <= 0 || req.signal.aborted)
        throw Error("Request deadline exceeded");
      const timeout = AbortSignal.timeout(Math.min(10_000, remaining));
      return fetcher(url, {
        ...init,
        cache: "no-store",
        redirect: "error",
        signal:
          typeof AbortSignal.any === "function"
            ? AbortSignal.any([req.signal, timeout])
            : timeout,
      });
    };

    if (token) {
      let userId: string;
      try {
        const response = await upstream(
          new URL(authBase.href.replace(/\/$/, "") + "/auth/v1/user"),
          {
            headers: {
              apikey: config.anonKey,
              Authorization: "Bearer " + token,
            },
          },
        );
        if (!response.ok)
          return reply(
            { error: "로그인이 만료됐어요.", code: "INVALID_SESSION" },
            401,
          );
        const user = await readJson(response, 65536);
        if (
          typeof user.id !== "string" ||
          !UUID.test(user.id) ||
          !user.email_confirmed_at
        )
          return reply(
            {
              error: "이메일 인증을 완료해주세요.",
              code: "UNVERIFIED_ACCOUNT",
            },
            403,
          );
        userId = user.id;
      } catch {
        return reply(
          {
            error: "로그인 상태를 확인하지 못했어요.",
            code: "AUTH_UNAVAILABLE",
          },
          503,
        );
      }
      if (config.multiSchool) {
        try {
          const response = await upstream(
            new URL("/api/schools/memberships", central),
            { headers: { Authorization: "Bearer " + token } },
          );
          if (!response.ok)
            return reply(
              {
                error: "학교 소속을 다시 확인해주세요.",
                code: "SCHOOL_ACCESS_DENIED",
              },
              response.status === 401 ? 401 : 403,
            );
          const data = await readJson(response, 131072);
          if (
            typeof data.current_school_id !== "string" ||
            !/^[-a-zA-Z0-9_]{1,100}$/.test(data.current_school_id) ||
            !Array.isArray(data.items)
          )
            throw Error();
          const membership = data.items.find(
            (m) =>
              record(m) &&
              m.user_id === userId &&
              m.school_id === data.current_school_id &&
              m.status === "ACTIVE" &&
              m.verified === true &&
              record(m.school) &&
              m.school.id === data.current_school_id &&
              m.school.status === "APPROVED",
          );
          if (!membership)
            return reply(
              {
                error: "인증된 학교를 선택해주세요.",
                code: "VERIFIED_SCHOOL_REQUIRED",
              },
              403,
            );
          const selectedSchool = (membership as Record<string, unknown>).school;
          if (
            !record(selectedSchool) ||
            !boundedText(selectedSchool.official_name, 120) ||
            !selectedSchool.official_name
          )
            throw Error("Invalid school name");
          schoolName = selectedSchool.official_name;
          schoolId = data.current_school_id;
        } catch {
          return reply(
            {
              error: "학교 소속을 확인하지 못했어요.",
              code: "SCHOOL_UNAVAILABLE",
            },
            503,
          );
        }
      } else {
        try {
          const response = await upstream(
            new URL(
              authBase.href.replace(/\/$/, "") +
                "/rest/v1/rpc/cbgpu_self_session_status",
            ),
            {
              method: "POST",
              headers: {
                apikey: config.anonKey,
                Authorization: "Bearer " + token,
                "Content-Type": "application/json",
              },
              body: "{}",
            },
          );
          if (!response.ok)
            return reply(
              {
                error: "로그인 상태 확인 연결을 준비하고 있어요.",
                code: "SESSION_VERIFICATION_UNAVAILABLE",
              },
              403,
            );
          const status = await readJson(response, 4096);
          if (status.valid !== true)
            return reply(
              { error: "로그인이 만료됐어요.", code: "INVALID_SESSION" },
              401,
            );
          if (status.approved !== true || status.school_id !== "cbsh")
            return reply(
              {
                error: "인증된 교내 계정만 이용할 수 있어요.",
                code: "SCHOOL_ACCESS_DENIED",
              },
              403,
            );
        } catch {
          return reply(
            {
              error: "로그인 상태 확인 연결을 준비하고 있어요.",
              code: "SESSION_VERIFICATION_UNAVAILABLE",
            },
            403,
          );
        }
      }
    }
    if (query.has("school_id") && query.get("school_id") !== schoolId)
      return reply(
        {
          error: "현재 선택한 학교와 요청한 학교가 달라요.",
          code: "SCHOOL_CONTEXT_CHANGED",
        },
        409,
      );
    const endpoint = new URL("/api/timetable", central);
    endpoint.searchParams.set("grade", grade);
    endpoint.searchParams.set("class", cls);
    if (query.get("refresh") === "1") endpoint.searchParams.set("refresh", "1");
    if (config.multiSchool) endpoint.searchParams.set("school_id", schoolId);
    try {
      const response = await upstream(endpoint, {
        headers: token ? { Authorization: "Bearer " + token } : {},
      });
      if (!response.ok)
        return reply(
          {
            error: "학교 시간표를 잠시 불러오지 못했어요.",
            code: "TIMETABLE_UNAVAILABLE",
          },
          response.status === 401 ? 401 : response.status === 403 ? 403 : 503,
        );
      const body = await readJson(response);
      if (config.multiSchool && body.school_id !== schoolId)
        throw Error("School mismatch");
      return reply(widgetTimetableDto(body, schoolId, publicOnly, schoolName));
    } catch {
      return reply(
        {
          error: "시간표 제공처 연결을 다시 확인하고 있어요.",
          code: "TIMETABLE_UNAVAILABLE",
        },
        503,
      );
    }
  };
}
