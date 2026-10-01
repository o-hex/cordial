/**
 * The App key and the JWT it signs.
 *
 * The PKCS#1 wrapping is the part worth testing and the easy part to fake: a
 * test that derived the PKCS#1 form by inverting `pkcs1ToPkcs8` would be
 * checking the code against itself. So `openssl` is the oracle -- it emits
 * both encodings of one key -- and the assertion is that both import and that
 * what they sign verifies against the *same* public key.
 */
import { assert, assertEquals, assertStringIncludes } from "jsr:@std/assert@^1.0.8";
import { appJwt, importAppKey } from "./github.ts";

async function run(args: string[], stdin?: string): Promise<string> {
  const command = new Deno.Command("openssl", {
    args,
    stdin: stdin === undefined ? "null" : "piped",
    stdout: "piped",
    stderr: "piped",
  });
  const child = command.spawn();
  if (stdin !== undefined) {
    const writer = child.stdin.getWriter();
    await writer.write(new TextEncoder().encode(stdin));
    await writer.close();
  }
  const { code, stdout, stderr } = await child.output();
  if (code !== 0) throw new Error(`openssl ${args.join(" ")}: ${new TextDecoder().decode(stderr)}`);
  return new TextDecoder().decode(stdout);
}

async function keyPair() {
  const pkcs8 = await run(["genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048"]);
  const pkcs1 = await run(["rsa", "-traditional"], pkcs8);
  const publicPem = await run(["rsa", "-pubout"], pkcs8);
  return { pkcs8, pkcs1, publicPem };
}

function derFromPem(pem: string): Uint8Array<ArrayBuffer> {
  const binary = atob(pem.replace(/-----[^-]+-----/g, "").replace(/\s+/g, ""));
  const out = new Uint8Array(new ArrayBuffer(binary.length));
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}

function unb64url(text: string): Uint8Array<ArrayBuffer> {
  const padded = text.replace(/-/g, "+").replace(/_/g, "/") +
    "=".repeat((4 - (text.length % 4)) % 4);
  const binary = atob(padded);
  const out = new Uint8Array(new ArrayBuffer(binary.length));
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}

Deno.test("both PEM encodings GitHub and openssl produce are accepted", async () => {
  const { pkcs8, pkcs1, publicPem } = await keyPair();
  assertStringIncludes(pkcs1, "BEGIN RSA PRIVATE KEY", "the oracle must emit PKCS#1");
  assertStringIncludes(pkcs8, "BEGIN PRIVATE KEY", "the oracle must emit PKCS#8");

  const verifier = await crypto.subtle.importKey(
    "spki",
    derFromPem(publicPem),
    { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" },
    false,
    ["verify"],
  );

  // The real assertion: a JWT signed through *either* import verifies against
  // the one public key. A wrapping that produced a different key would import
  // cleanly and fail here, which is the failure mode worth catching.
  for (const [shape, pem] of [["PKCS#8", pkcs8], ["PKCS#1", pkcs1]] as const) {
    const jwt = await appJwt("123456", await importAppKey(pem), 1_700_000_000_000);
    const [head, body, signature] = jwt.split(".");
    assert(
      await crypto.subtle.verify(
        "RSASSA-PKCS1-v1_5",
        verifier,
        unb64url(signature),
        new TextEncoder().encode(`${head}.${body}`),
      ),
      `a JWT from the ${shape} import must verify against the same public key`,
    );
  }
});

Deno.test("the JWT says what GitHub requires, and backdates iat", async () => {
  const { pkcs8 } = await keyPair();
  const now = 1_700_000_000_000;
  const jwt = await appJwt("123456", await importAppKey(pkcs8), now);
  const [head, body] = jwt.split(".").slice(0, 2)
    .map((p) => JSON.parse(new TextDecoder().decode(unb64url(p))));

  assertEquals(head, { alg: "RS256", typ: "JWT" });
  assertEquals(body.iss, "123456");
  // Backdated, because GitHub refuses a token issued in its own future and a
  // host clock a few seconds fast is the ordinary way that happens.
  assertEquals(body.iat, now / 1000 - 60);
  // And inside GitHub's ten-minute ceiling, measured from the real issue time
  // rather than from the backdated one.
  assert(body.exp - now / 1000 <= 10 * 60, `exp is ${body.exp - now / 1000}s away`);
  assert(body.exp > now / 1000);
});

Deno.test("a PEM that came through an environment variable still imports", async () => {
  const { pkcs8, pkcs1, publicPem } = await keyPair();
  const verifier = await crypto.subtle.importKey(
    "spki",
    derFromPem(publicPem),
    { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" },
    false,
    ["verify"],
  );

  // How a PEM actually arrives from a hosting provider's env: one line, real
  // newlines replaced by a backslash and an `n`. The `n` is valid base64, so a
  // whitespace strip leaves it inside the body and `atob` fails -- which is
  // exactly how the first real deployment of this bridge died, with an error
  // naming neither the key nor the escaping.
  for (const [shape, pem] of [["PKCS#8", pkcs8], ["PKCS#1", pkcs1]] as const) {
    const escaped = pem.replace(/\n/g, "\\n");
    assert(!escaped.includes("\n"), "the fixture must be single-line to be the real case");

    const jwt = await appJwt("123456", await importAppKey(escaped), 1_700_000_000_000);
    const [head, body, signature] = jwt.split(".");
    assert(
      await crypto.subtle.verify(
        "RSASSA-PKCS1-v1_5",
        verifier,
        unb64url(signature),
        new TextEncoder().encode(`${head}.${body}`),
      ),
      `an escaped ${shape} key must import and sign identically`,
    );
  }
});

Deno.test("CRLF escapes survive too, since Windows-written env files carry them", async () => {
  const { pkcs8 } = await keyPair();
  const escaped = pkcs8.replace(/\n/g, "\\r\\n");
  const jwt = await appJwt("1", await importAppKey(escaped), 1_700_000_000_000);
  assertEquals(jwt.split(".").length, 3);
});

// ---- labels, and the calls the editor makes -----------------------------------

import { GitHub } from "./github.ts";

/** Run `body` with `fetch` replaced, and hand back what was requested. */
async function withFetch(
  respond: (url: string, init: RequestInit) => unknown,
  body: (requests: { url: string; init: RequestInit }[]) => Promise<void>,
) {
  const real = globalThis.fetch;
  const requests: { url: string; init: RequestInit }[] = [];
  globalThis.fetch = ((input: string | URL | Request, init: RequestInit = {}) => {
    const url = String(input);
    requests.push({ url, init });
    return Promise.resolve(Response.json(respond(url, init)));
  }) as typeof fetch;
  try {
    await body(requests);
  } finally {
    globalThis.fetch = real;
  }
}

const client = () => new GitHub({ owner: "o", repo: "r" }, () => Promise.resolve("t"));

Deno.test("labels are read a hundred to a page until a short page ends it", async () => {
  const page = (n: number) =>
    Array.from({ length: n }, (_, i) => ({ name: `l${i}`, description: i ? null : "first" }));
  await withFetch(
    (url) => (url.endsWith("&page=1") ? page(100) : page(7)),
    async (requests) => {
      const labels = await client().labels();
      assertEquals(labels.length, 107);
      assertEquals(labels[0], { name: "l0", description: "first" });
      assertEquals(requests.length, 2);
      assertStringIncludes(requests[0].url, "/repos/o/r/labels?per_page=100&page=1");
      assertStringIncludes(requests[1].url, "page=2");
    },
  );
});

Deno.test("label paging is bounded, so a misbehaving API cannot hang an interaction", async () => {
  await withFetch(
    () => Array.from({ length: 100 }, (_, i) => ({ name: `l${i}` })),
    async (requests) => {
      await client().labels();
      assertEquals(requests.length, 10);
    },
  );
});

Deno.test("an issue's labels come back as names, whichever shape GitHub sent", async () => {
  await withFetch(
    () => ({
      title: "t",
      state: "open",
      body: "b",
      labels: [{ name: "bug" }, "area:input", { name: "" }],
    }),
    async () => {
      assertEquals((await client().issue(3)).labels, ["bug", "area:input"]);
    },
  );
});

Deno.test("an edit is one PATCH carrying only what changed", async () => {
  await withFetch(() => ({}), async (requests) => {
    await client().updateIssue(3, { title: "new", labels: ["a"] });
    assertEquals(requests[0].init.method, "PATCH");
    assertStringIncludes(requests[0].url, "/repos/o/r/issues/3");
    assertEquals(JSON.parse(String(requests[0].init.body)), { title: "new", labels: ["a"] });
  });
});

Deno.test("a comment returns its id so an audit entry can be amended", async () => {
  await withFetch(() => ({ id: 55, html_url: "https://x/y" }), async (requests) => {
    assertEquals(await client().comment(3, "hi"), { id: 55, html_url: "https://x/y" });
    await client().editComment(55, "amended");
    assertEquals(requests[1].init.method, "PATCH");
    assertStringIncludes(requests[1].url, "/repos/o/r/issues/comments/55");
  });
});
