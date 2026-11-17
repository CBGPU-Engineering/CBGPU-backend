// Thin compatibility URL for released widgets. No duplicate provider decoder or cross-school cache.
import { createTimetableProxy } from "./proxy.ts";
Deno.serve(
  createTimetableProxy({
    centralOrigin:
      Deno.env.get("CBGPU_TIMETABLE_CENTRAL_ORIGIN") || "https://cbgpu.com",
    privateContainer: Deno.env.get("CBGPU_TIMETABLE_PRIVATE_CONTAINER") === "1",
    supabaseUrl: Deno.env.get("SUPABASE_URL") || "",
    anonKey: Deno.env.get("SUPABASE_ANON_KEY") || "",
    multiSchool: Deno.env.get("CBGPU_MULTISCHOOL_V1") === "1",
  }),
);
