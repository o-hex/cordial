import { assert, assertEquals } from "jsr:@std/assert@^1.0.8";
import {
  canEditReport,
  holdsModeratorPermission,
  isModerator,
  parseRoleIds,
} from "./permissions.ts";

const MANAGE_MESSAGES = String(1n << 13n);
const MANAGE_THREADS = String(1n << 34n);
const ADMINISTRATOR = String(1n << 3n);
const SEND_MESSAGES = String(1n << 11n);

Deno.test("Manage Messages, Manage Threads and Administrator make a moderator", () => {
  for (const bits of [MANAGE_MESSAGES, MANAGE_THREADS, ADMINISTRATOR]) {
    assert(isModerator({ permissions: bits }), bits);
  }
});

Deno.test("an ordinary member, or a bad bitfield, is not one", () => {
  assert(!isModerator({ permissions: SEND_MESSAGES }));
  assert(!isModerator({ permissions: "not a number" }), "malformed reads as no permission");
  assert(!isModerator(undefined));
  assert(!holdsModeratorPermission(undefined));
});

Deno.test("a configured role makes a moderator of somebody with no permission bits", () => {
  assert(isModerator({ permissions: SEND_MESSAGES, roles: ["10", "20"] }, ["20"]));
  assert(!isModerator({ permissions: SEND_MESSAGES, roles: ["10"] }, ["20"]));
});

Deno.test("a role only ever widens: an administrator is a moderator whatever the list says", () => {
  assert(isModerator({ permissions: ADMINISTRATOR, roles: [] }, ["999"]));
});

Deno.test("role ids parse from commas or spaces, and a typo is reported rather than skipped", () => {
  assertEquals(parseRoleIds("123, 456 789"), { ids: ["123", "456", "789"], invalid: [] });
  assertEquals(parseRoleIds("123,@Moderators"), { ids: ["123"], invalid: ["@Moderators"] });
  assertEquals(parseRoleIds(undefined), { ids: [], invalid: [] });
});

Deno.test("the filer may edit, a moderator may edit, a stranger may not", () => {
  assert(canEditReport("9", "9", false), "the original poster");
  assert(canEditReport("1", "9", true), "a moderator who did not file it");
  assert(!canEditReport("1", "9", false), "anybody else");
});

Deno.test("an issue with no recorded filer can be edited only by a moderator", () => {
  assert(!canEditReport("9", null, false));
  assert(canEditReport("9", null, true));
  assert(!canEditReport(undefined, "9", false));
});
