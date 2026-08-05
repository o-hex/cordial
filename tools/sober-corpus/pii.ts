/**
 * PII redaction for the Sober issue corpus.
 *
 * These are real third-party users' bug reports, routinely pasted verbatim
 * from a terminal: `inxi`/`neofetch`/`journalctl` dumps, stack traces, shell
 * history. That means email addresses, absolute home paths
 * (`/home/<username>/...`), IP addresses, and shell-prompt/`Host:`-style
 * machine hostnames show up constantly. Everything below is redacted on
 * write, before anything ever lands in raw.jsonl — Cordial is a public repo
 * and this corpus is other people's bug reports with no license grant to us,
 * so it stays local (see the repo `.gitignore` entry for this directory's
 * `data/`) and scrubbed regardless.
 *
 * `redactEmbeddedSecrets` below matches secret-shaped substrings — Bearer
 * tokens, provider-prefixed API keys, JWTs, URL userinfo credentials —
 * anywhere inside a longer string, rather than only when the whole value IS
 * one. A whole-value check would miss "auth failed for Bearer sk_live_…"
 * entirely, and a bug report is exactly the kind of prose that pastes an
 * error message with a credential in the middle. It is deliberately narrow —
 * provider-prefixed keys and known token shapes only, not every 24-char
 * hex-looking run — because a bug report is full of commit hashes, build
 * numbers and trace ids that are not secrets and are the diagnostic content
 * this corpus exists to keep.
 *
 * The PII patterns (email, home path, IPv4, hostname) are a separate,
 * broader pass and run after the secret patterns — narrower patterns first,
 * so a broad PII pattern cannot eat into an already-redacted secret token.
 *
 * This is a best-effort, pattern-based scrubber, not a guarantee: freeform
 * pasted logs can leak PII in shapes no regex list fully covers (e.g. a
 * hostname embedded in `uname -a` output with no anchoring keyword). It
 * covers the shapes actually observed in this repo's issues.
 *
 * @module sober-corpus/pii
 */

// `Bearer <token>` / `Basic <token>` anywhere in the string.
const BEARER_OR_BASIC_PATTERN = /\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]{8,}/gi;

// Provider-prefixed API keys: Stripe/OpenAI `sk_`/`pk_`, GitHub
// `ghp_`/`gho_`/`ghu_`/`ghs_`/`ghr_`, and their kin. Keeps the prefix so the
// redacted text still says which kind of credential was there.
const PROVIDER_KEY_PATTERN = /\b(sk|pk|rk|re|ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9_-]{12,}/g;

// Slack `xoxb-`/`xoxp-`/...
const SLACK_TOKEN_PATTERN = /\bxox[bpsoae]-[A-Za-z0-9-]{10,}/g;

// JWTs (three base64url segments) — replaced whole; no part of a JWT is safe
// to keep, the header and payload are only base64, not encrypted.
const JWT_PATTERN = /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}/g;

// Credentials in a URL userinfo component, e.g. `postgres://user:pw@host`.
// The host is deliberately kept — that is the diagnostic half.
const URL_USERINFO_PATTERN = /:\/\/[^\s/@:]+:[^\s/@]+@/g;

/** Redacts secret-shaped substrings embedded anywhere in `text`, without touching the rest of it. */
function redactEmbeddedSecrets(text: string): string {
  return text
    .replace(BEARER_OR_BASIC_PATTERN, (m) => `${m.split(/\s+/)[0]} [redacted]`)
    .replace(PROVIDER_KEY_PATTERN, (m) => `${m.split("_")[0]}_[redacted]`)
    .replace(SLACK_TOKEN_PATTERN, (m) => `${m.slice(0, 4)}-[redacted]`)
    .replace(JWT_PATTERN, () => "[redacted-jwt]")
    .replace(URL_USERINFO_PATTERN, () => "://[redacted]@");
}

const EMAIL_PATTERN = /[\w.+-]+@[\w-]+\.[a-zA-Z]{2,}/g;

// `/home/<user>/...` — keep the rest of the path (it's diagnostic: which
// desktop file, which Flatpak data dir), redact only the username segment.
const HOME_PATH_PATTERN = /\/home\/[^/\s"'`]+/g;

// IPv4 only (0-255 per octet) to cut down on false positives against version
// strings, though some ambiguity with e.g. "1.2.3.4"-shaped build numbers is
// an accepted, documented limitation — see module docstring.
const IPV4_PATTERN = /\b(?:(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\.){3}(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\b/g;

// Shell-prompt style `user@host:` (e.g. pasted terminal output like
// `neil@fedora-box:~/Downloads$`). Redacts both the user and host segments,
// keeps the path/`$` that follows since that's diagnostic (cwd at time of error).
const SHELL_PROMPT_PATTERN = /\b[A-Za-z0-9_.-]+@[A-Za-z0-9_.-]+(?=:[~/])/g;

// `Host:`/`Hostname:`/`hostname=` lines as commonly pasted from
// `hostnamectl`/`inxi`/`neofetch` system-info dumps in bug reports.
const HOSTNAME_LINE_PATTERN = /^(\s*Host(?:name)?\s*[:=]\s*)(\S+)/gim;

function redactEmails(text: string): string {
  return text.replace(EMAIL_PATTERN, "[redacted-email]");
}

function redactHomePaths(text: string): string {
  return text.replace(HOME_PATH_PATTERN, "/home/[redacted-user]");
}

function redactIPv4(text: string): string {
  return text.replace(IPV4_PATTERN, "[redacted-ip]");
}

function redactShellPromptHosts(text: string): string {
  return text.replace(SHELL_PROMPT_PATTERN, "[redacted-user]@[redacted-host]");
}

function redactHostnameLines(text: string): string {
  return text.replace(HOSTNAME_LINE_PATTERN, "$1[redacted-host]");
}

/**
 * Redacts secrets and PII from a single piece of free-form issue/comment
 * text. Order matters: secrets first (narrower, more specific patterns),
 * then PII (broader patterns that could otherwise eat into an
 * already-redacted secret token).
 */
export function redactIssuePII(text: string): string {
  if (!text) {
    return text;
  }
  let redacted = redactEmbeddedSecrets(text);
  redacted = redactEmails(redacted);
  redacted = redactHomePaths(redacted);
  redacted = redactShellPromptHosts(redacted);
  redacted = redactHostnameLines(redacted);
  redacted = redactIPv4(redacted);
  return redacted;
}
