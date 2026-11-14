# Moving a developer's secrets to a second machine (MacBook to Ubuntu homelab over Tailscale) for development and coding-agent handoff

Research date: 2026-11-14. Sources fetched this session are linked inline. Statements without a link are under Inferences or Gaps, and those drawn from background knowledge rather than a source fetched this session are marked "(unverified this session)".

## Q1. Mechanics and security properties of each approach family (encrypted-in-repo, secret managers, direct transfer, runtime injection)

### Takeaway
There are three workable patterns:
1. Encrypt to the homelab's own key, so ciphertext can sit in git or travel anywhere: SOPS+age or dotenvx.
2. Keep secrets in a manager and inject them at runtime: 1Password `op run`, or self-hosted Infisical or Phase.
3. Copy the gitignored files directly over Tailscale SSH (scp/sftp).

For a solo developer, a bulk copy over Tailscale SSH followed by local encryption-at-rest (SOPS+age) is the simplest path for the bootstrap. A per-handoff tool is best built on an encrypted channel or on age-encrypted bundles.

### Cited Findings

**SOPS + age (encrypted-in-repo)**
- SOPS is "an editor of encrypted files that supports YAML, JSON, ENV, INI and BINARY formats." It encrypts with AWS KMS, GCP KMS, Azure Key Vault, HuaweiCloud KMS, age, and PGP. — [SOPS docs](https://getsops.io/docs/)
- SOPS's 1.X and 2.X branches are current, and the file format stays compatible within a major version. — [SOPS docs](https://getsops.io/docs/)
- Age key lookup: `$XDG_CONFIG_HOME/sops/age/keys.txt`. On Linux the fallback is `~/.config/sops/age/keys.txt`, and on macOS it is `~/Library/Application Support/sops/age/keys.txt`. `SOPS_AGE_KEY_FILE`, `SOPS_AGE_KEY`, and `SOPS_AGE_KEY_CMD` override it (`SOPS_AGE_KEY_CMD` runs a command that outputs the key). — [SOPS age docs](https://getsops.io/docs/usage/identities/age/)
- SOPS can encrypt to `ssh-ed25519` or `ssh-rsa` public keys as age recipients. Decryption checks `SOPS_AGE_SSH_PRIVATE_KEY_FILE`, then `SOPS_AGE_SSH_PRIVATE_KEY_CMD` (the key must not be passphrase-protected), then `~/.ssh/id_ed25519`, then `~/.ssh/id_rsa`. — [SOPS age docs](https://getsops.io/docs/usage/identities/age/)
- `.sops.yaml` `creation_rules` can list several age recipients separated by commas, for example a laptop key and a homelab key. — [SOPS age docs](https://getsops.io/docs/usage/identities/age/)

**dotenvx (encrypted .env)**
- dotenvx positions itself as a way to "encrypt, commit, and ship" env files. It has `encrypt`, `run`, `run --redact` ("Runtime leak protection & log redaction"), `check`, and "Armor" / "Local" key-custody options. — [dotenvx docs index](https://dotenvx.com/docs/introduction/)
- Keys: secp256k1 is the public-key algorithm. The public key sits in `.env` as `DOTENV_PUBLIC_KEY`. The private key goes in `.env.keys`, as `DOTENV_PRIVATE_KEY` for `.env` and `DOTENV_PRIVATE_KEY_PRODUCTION` for `.env.production`. At runtime the key comes from `.env.keys` or from a `DOTENV_PRIVATE_KEY*` env var. — [dotenvx .env.keys docs](https://dotenvx.com/docs/env-keys-file/) (via search summary)
- Encryption is ECIES (ephemeral ECDH plus AES-256-GCM) per value. — [dotenvx crate README on docs.rs](https://docs.rs/crate/dotenvx/0.1.0). Caveat: this is an independent Rust port, not the official CLI, so treat it as a description of the scheme rather than an authoritative spec.
- The `encrypt` CLI page lists flags including `-f`, `-k`, `-ek`, `--stdout`, `--no-armor`, and `--no-native`, but doesn't explain them. — [dotenvx encrypt](https://dotenvx.com/docs/cli/encrypt)

**1Password CLI and 1Password Environments**
- `op run --env-file=./prod.env -- cmd` resolves `op://vault/item/field` references in an env file. Secrets reach the subprocess "only for the duration of the process", and the env file holds references, not values. The page warns that "processes on your computer can access the environment of other processes run by the same user." — [1Password CLI: secrets in env vars](https://www.1password.dev/cli/secrets-environment-variables/)
- `op run --environment <environmentID> -- <command>` loads a 1Password Environment, and it can be combined with `--env-file`. Service accounts can be "scoped to specific vaults and 1Password Environments." — [1Password CLI: secrets in env vars](https://www.1password.dev/cli/secrets-environment-variables/)
- 1Password Environments, as of Nov 2026:
  - **What it is:** a per-project or per-stage secret set. It offers "a file 1Password mounts for your app, with nothing written to disk". The page doesn't describe the mechanism.
  - **Platforms:** the desktop app on Mac, Windows, and Linux. Not on iOS or Android.
  - **Admin:** needs an admin-enabled Environments policy.
  - **Integrations:** an MCP server for agents such as Claude Code and Cursor, plus AWS Secrets Manager sync (beta).
  - **Values:** hidden by default and masked in CLI and SDK output.
  - **Deletion:** a deleted Environment can't be restored.
  - **Status:** the page doesn't label Environments as beta or GA.
  - Source: [1Password Environments](https://www.1password.dev/environments/)

**Self-hostable secret managers**
- Infisical:
  - **Deployment:** self-hosted with Docker Compose (`docker-compose.prod.yml`, `docker compose up -d`). A third-party guide gives 4 GB RAM minimum on Ubuntu, Postgres backups via `pg_dump`, and advises turning off public signup. — [OneUptime guide, Mar 2026](https://oneuptime.com/blog/post/2026-03-02-how-to-configure-infisical-for-secret-management-on-ubuntu/markdown)
  - **Infisical's own homelab write-up:** reach the instance over Tailscale instead of exposing it, have each stack fetch its credentials at startup, and expect no limits on projects or environments when self-hosted. — [Infisical blog: self-hosting Infisical homelab](https://infisical.com/blog/self-hosting-infisical-homelab); [Infisical: Tailscale + Infisical video page](https://infisical.com/videos/tailscale-infisical-homelab-video-page)
  - **Licensing:** the core is open source, and extra enterprise features need a commercial license. — [OneUptime guide](https://oneuptime.com/blog/post/2026-03-02-how-to-configure-infisical-for-secret-management-on-ubuntu/markdown)
- Phase:
  - **License:** the console is MIT-licensed, except an `ee` directory that needs a Phase license. — [phasehq/console](https://github.com/phasehq/console)
  - **Deployment:** self-hosting with Docker Compose, Helm, AWS, or GCP is free. — [Phase docs](https://docs.phase.dev/)
  - **Headless CLI:** use a service account token and set `PHASE_HOST` for a self-hosted instance. Example: `docker run --env-file <(phase secrets export --app X --env production) …`. Docker `--env-file` doesn't support multi-line secrets. — [Phase Docker docs](https://docs.phase.dev/integrations/platforms/docker)
- Bitwarden Secrets Manager:
  - **Self-hosting:** limited to Enterprise customers. It runs as multiple Docker Linux containers with Microsoft SQL Server, or via Helm. — [Bitwarden blog, 2023](https://bitwarden.com/blog/enterprise-self-hosting-for-bitwarden-secrets-manager)
  - **Free tier:** unlimited secrets, 2 users, 3 projects, and 3 machine accounts. — [Bitwarden SM FAQ](https://bitwarden.com/en-gb/help/secrets-manager-faqs/)
  - These sources are from 2023–24.

**Direct transfer over Tailscale**
- Tailscale SSH:
  - **Authentication:** it authenticates with WireGuard node keys, uses SSH `none` auth, and needs no `authorized_keys`.
  - **Check mode:** `action: "check"` with `checkPeriod` (default 12h, 1 min to 168h, or `always`) forces IdP re-auth, which can include MFA.
  - **File transfer:** "Tailscale implements … SFTP which lets SCP and SFTP work". rsync isn't mentioned.
  - **Revocation:** takes effect within seconds and kills existing sessions.
  - **Caveat:** any OS user on the client can connect, so it may be a poor fit for machines running untrusted code. Restarting `tailscaled` ends sessions.
  - Source: [Tailscale SSH KB](https://tailscale.com/kb/1193/tailscale-ssh)
- magic-wormhole and croc:
  - **magic-wormhole:** uses SPAKE2 PAKE, so the sender and receiver don't need to trust the mailbox or relay servers. — [Magic Wormhole (Wikipedia)](https://en.wikipedia.org/wiki/Magic_Wormhole); [IETF CFRG PAKE review](https://mailarchive.ietf.org/arch/msg/cfrg/BBQ2gwCECu5ouTJjE_CE6d9Rg-0)
  - **croc:** also uses PAKE, supports a self-hosted relay (`croc relay`), and supports resume. One forum commenter raised unverified security concerns about croc's repo issues. — [croc guide](https://kx.cloudingenium.com/en/croc-file-transfer-secure-any-two-computers-guide/); [HN thread](https://hn.svelte.dev/item/45448747)

**Runtime injection without files**
- systemd credentials:
  - `systemd-creds encrypt` produces ciphertext that a unit loads with `LoadCredentialEncrypted=`, or embeds with `SetCredentialEncrypted=`.
  - The key can be TPM2, a host secret at `/var/lib/systemd/credential.secret`, or both. — [systemd CREDENTIALS.md](https://third-party-mirror.googlesource.com/systemd/+/refs/heads/master/docs/CREDENTIALS.md)
  - Encrypt and decrypt have existed since systemd 250. — [systemd-creds(1)](https://man7.org/linux/man-pages/man1/systemd-creds.1.html)
  - A TPM2-bound credential can only be decrypted on the same machine. — [systemd-creds.xml](https://third-party-mirror.googlesource.com/systemd/+/refs/heads/master/man/systemd-creds.xml)

### Inferences
- **SOPS+age** is the strongest encrypted-in-repo choice for this user.
  - Plain dotenv support keeps `.env` shape.
  - Multi-recipient rules (laptop + homelab + offline backup key) give per-machine revocation via `sops updatekeys`. That command wasn't fetched this session; it's in SOPS's CLI (unverified this session).
  - Encrypting to the homelab's existing ssh-ed25519 host or user key means no new key distribution.
  - Caveat: with SOPS on dotenv files, key names stay plaintext and values are encrypted.
- **dotenvx** gives a single-command DX (`dotenvx run -- pnpm dev`), but it uses one keypair per env file. The homelab must then receive `.env.keys` or `DOTENV_PRIVATE_KEY`, which is itself a secret transfer, so the transfer problem only moves.
- **git-crypt and transcrypt** (unverified this session):
  - git-crypt uses a symmetric repo key, optionally wrapped with GPG. Revoking a machine means rotating the key and re-encrypting.
  - transcrypt uses a shared OpenSSL password.
  - Both are weaker fits than SOPS+age because there's no per-recipient revocation, and both put encrypted blobs in git history forever, which raises the stakes of an eventual key leak.
- **Doppler and Teller** (unverified this session):
  - Doppler is SaaS-only, with no self-hosting.
  - Teller (tellerops/teller) is a multi-provider fetcher. Its maintenance status couldn't be confirmed (see Gaps).
- **Taildrop** (not fetched this session): Taildrop sends files between your own tailnet devices, but it is interactive and lands files in a download inbox, so it suits ad-hoc transfers more than scripted per-project placement.
- **Tailscale SSH vs PAKE tools:** Tailscale SSH plus scp/sftp, or `tar | ssh homelab tar x`, needs no third-party relay. That makes it strictly better than magic-wormhole or croc for two machines already on the same tailnet. PAKE tools matter only when no authenticated channel exists.

### Gaps
- No primary-source detail on how 1Password Environments' "mounted" `.env` works on Linux (named pipe / FUSE?), or whether it works headless without the desktop app. The page didn't say, and a headless homelab likely needs a service account plus `op run --environment` instead.
- Not fetched: the official dotenvx whitepaper with exact crypto, the SOPS `updatekeys`/`rotate` docs, git-crypt, transcrypt, Doppler, HashiCorp Vault and OpenBao self-hosting docs, the Teller repo status, Tailscale Taildrop docs, envchain, and pass/secret-service.
- Bitwarden SM self-hosting sources are 2023–24; current terms weren't verified.

## Q2. Coding-agent specifics: Claude Code credentials, headless auth, keeping secrets out of the context window and transcripts

### Takeaway
Claude Code keeps its login in `~/.claude/.credentials.json` (mode 0600) on Linux and in the Keychain on macOS. A one-year `CLAUDE_CODE_OAUTH_TOKEN` from `claude setup-token` works headless, but it **cannot establish Remote Control sessions**. That matters because this user's homelab tabs run `claude rc`, so the homelab needs a real `/login` (`claude auth login`, with the copy-URL / paste-code flow).

To keep secrets out of the model's context, combine three things:
- `Read` deny rules (they cover the file tools and recognized Bash commands like `cat`)
- sandbox `denyRead` and `credentials` deny or mask entries (OS-level, so they cover arbitrary subprocesses)
- an explicit secret scan of transcripts before transfer

### Cited Findings
- **Storage:**
  - macOS uses the encrypted Keychain. If the Keychain rejects the write (for example when it's locked in an SSH session), Claude Code falls back to `~/.claude/.credentials.json` (mode 0600).
  - Linux uses `~/.claude/.credentials.json` (0600).
  - `CLAUDE_CONFIG_DIR` relocates the file and keys the Keychain entry per directory.
  - Source: [Claude Code authentication](https://code.claude.com/docs/en/authentication)
- **Headless login:** `claude auth login` before the first session skips the prompt. If no browser opens, press `c` to copy the login URL, and paste the code back at "Paste code here if prompted". — [Claude Code authentication](https://code.claude.com/docs/en/authentication)
- **Precedence**, highest first:
  1. Cloud provider env
  2. `ANTHROPIC_AUTH_TOKEN`
  3. `ANTHROPIC_API_KEY` (asks for approval once in interactive mode, always used in `-p`)
  4. `apiKeyHelper` script
  5. `CLAUDE_CODE_OAUTH_TOKEN`
  6. Anthropic profiles / WIF
  7. Subscription OAuth from `/login`

  Source: [Claude Code authentication](https://code.claude.com/docs/en/authentication)
- **`claude setup-token`:**
  - It mints a one-year OAuth token, prints it, and doesn't save it.
  - It "can only make model requests, so it can't establish Remote Control sessions or fetch claude.ai connectors."
  - Bare mode (`--bare`) doesn't read it.
  - Source: [Claude Code authentication](https://code.claude.com/docs/en/authentication)
- **`apiKeyHelper`** runs a shell script that returns a key, which suits short-lived keys fetched from a vault. Settings reload live. — [Claude Code authentication](https://code.claude.com/docs/en/authentication); [Claude Code settings](https://code.claude.com/docs/en/settings)
- **Renewal:** logins warn 3 days before expiry. Remote Control or background sessions that outlive the login stop making progress. — [Claude Code authentication](https://code.claude.com/docs/en/authentication)
- **`Read` deny rules:** the documented example is `"deny": ["Read(./.env)", "Read(./.env.*)"]`, and `.claudeignore` has no effect. — [Claude Code settings](https://code.claude.com/docs/en/settings); [Claude Code permissions](https://code.claude.com/docs/en/permissions)
- **Limits of `Read` deny rules:** they apply to the file tools, to Bash file commands Claude Code recognizes (`cat`, `head`, `tail`, `sed`, `tee`), and to redirection targets. They do **not** apply to `grep -r pattern .`, or to scripts (Python or Node) that open files themselves: "For OS-level enforcement … enable the sandbox." — [Claude Code permissions](https://code.claude.com/docs/en/permissions)
- **Rule anchoring:** a bare `Read(.env)` matches at any depth under the cwd, and `Read(//**/.env)` matches anywhere on the filesystem. Deny rules apply right away and don't wait for workspace trust. — [Claude Code permissions](https://code.claude.com/docs/en/permissions); [Claude Code settings](https://code.claude.com/docs/en/settings)
- **Sandbox:**
  - Default reads are permissive ("Read access to the entire computer"), and there is "no built-in credential deny list".
  - `sandbox.filesystem.denyRead` and `allowRead` control reads.
  - `sandbox.credentials.files` with `mode: "deny"` covers paths such as `~/.claude/.credentials.json` and `~/.ssh`.
  - `sandbox.credentials.envVars` deny unsets env vars before each sandboxed command.
  - `mode: "mask"` shows a placeholder that the proxy swaps for the real value on allowed hosts. It requires `network.tlsTerminate`.
  - Linux uses bubblewrap and macOS uses Seatbelt.
  - `denyRead` does not stop the built-in Read tool, so it must be paired with `Read` deny rules.
  - Source: [Claude Code sandboxing](https://code.claude.com/docs/en/sandboxing)
- **Session history location:** `CLAUDE_CONFIG_DIR` holds settings, **session history**, and the login. — [Claude Code authentication](https://code.claude.com/docs/en/authentication); [Claude Code settings](https://code.claude.com/docs/en/settings)
- **gitleaks:**
  - `gitleaks dir <path>` and `cat file | gitleaks stdin`.
  - `--redact[=N]` redacts secrets in logs and stdout.
  - Reports: json, csv, junit, sarif, or a Go template.
  - Status: "feature complete", security patches only, with the author moving to **Betterleaks**.
  - Source: [gitleaks README](https://github.com/gitleaks/gitleaks)
- **TruffleHog:**
  - `trufflehog filesystem <paths>` and `trufflehog stdin`.
  - `--results=verified` keeps only secrets confirmed live via provider APIs, and `--no-verification` disables those network checks.
  - The README gives two counts: 700+ and 800+ detector types.
  - Source: [TruffleHog README](https://github.com/trufflesecurity/trufflehog)

### Inferences
- **Remote Control constraint:** because `CLAUDE_CODE_OAUTH_TOKEN` can't do Remote Control, a handoff tool that resumes a session via `claude rc` on the homelab must rely on a homelab-resident `/login` credential. Copying `~/.claude/.credentials.json` between machines is undocumented. A macOS laptop normally holds it in the Keychain, not a file, and sharing one OAuth refresh token across two machines may break token refresh if Anthropic rotates refresh tokens (unverified). Recommendation: run `claude auth login` once on the homelab (bootstrap), and have the handoff tool only **verify** auth there (`claude auth status`, or `/status`), never ship it.
- **Context-window exposure:** an agent that `cat`s or greps `.env`, or runs a script that prints env vars, puts the values into the conversation. Conversation JSONL is stored on disk under the config dir (typically `~/.claude/projects/<slug>/*.jsonl`; this exact path wasn't verified this session). A transferred transcript can therefore carry plaintext secrets.
- **Handoff tool transcript handling:**
  - (a) Scan the transcript with `gitleaks stdin --redact` or `trufflehog filesystem --no-verification` before sending, so the scan doesn't send candidate secrets to provider APIs.
  - (b) Refuse, or redact in place, on findings.
  - (c) Send it only over Tailscale SSH, never through a third-party relay.
- **Inject-at-runtime tools reduce but don't remove leakage:** `op run` / `dotenvx run` / `phase run` keep secrets off disk, but an agent can still print `env` from inside the process. Pair them with `credentials.envVars` deny or mask in the sandbox. dotenvx's `run --redact` (log redaction) is a further mitigation, but its exact behavior wasn't verified this session.

### Gaps
- Whether `~/.claude/.credentials.json` can be safely copied between hosts, and whether refresh-token rotation invalidates the source machine. No source found.
- The official transcript path and whether Claude Code itself redacts secrets in transcripts. Not verified this session.
- Betterleaks (the gitleaks successor) wasn't researched.

## Q3. Discovery: automatically finding which gitignored files are secrets a project needs

### Takeaway
List ignored-but-present files with git, filter them by known secret filename patterns, then confirm contents with gitleaks or TruffleHog. For SOPS-encrypted files, a "known secret files" manifest per repo (or a `.sops.yaml` path regex) is the authoritative list.

### Cited Findings
- gitleaks `dir` and TruffleHog `filesystem` can scan arbitrary files outside git history, which is how gitignored files get content-checked. — [gitleaks README](https://github.com/gitleaks/gitleaks); [TruffleHog README](https://github.com/trufflesecurity/trufflehog)
- Claude Code `Read` deny rules use gitignore semantics, including `!` negation such as `Read(*.env)` then `Read(!sample.env)`. The same pattern list can drive both discovery and agent deny rules. — [Claude Code permissions](https://code.claude.com/docs/en/permissions)
- `.sops.yaml` `creation_rules` select recipients per path. — [SOPS age docs](https://getsops.io/docs/usage/identities/age/)

### Inferences
- **Practical discovery pipeline** (standard git, unverified this session):
  - `git ls-files --others --ignored --exclude-standard -z` lists ignored-but-present files.
  - Exclude dependency and build dirs (`node_modules`, `.next`, `dist`, `.venv`, `target`).
  - Match against an allowlist of secret patterns:
    - `.env`, `.env.*` (minus `.env.example` / `.env.sample` / `.env.template`)
    - `.dev.vars` (Cloudflare Wrangler), `.envrc`
    - `*.pem`, `*.key`, `*.p12`, `*.p8`, `id_*`
    - `config/*.local.*`, `*.local.json`, `.npmrc`, `.netrc`
    - `credentials.json`, `service-account*.json`, `.firebaserc`-adjacent keys
    - `GoogleService-Info.plist`, `google-services.json`
    - `.claude/settings.local.json` (may hold an `env` block)
  - Then run `gitleaks dir` on the candidates to flag high-entropy content in non-obvious files.
- **Matching template:** if a committed `.env.example` exists, use its key names to check that the homelab copy has every required key, without reading values.

### Gaps
- No primary source found for a maintained tool that does "find gitignored secret files across many repos" as a single command.

## Q4. Recommendation and ranking for a solo dev (MacBook + Ubuntu homelab on Tailscale): one-time bootstrap and per-handoff building block

### Takeaway
**Bootstrap:** stream a tar of discovered gitignored secret files over Tailscale SSH into each repo on the homelab. Then (optionally) convert them to SOPS+age so ciphertext can live next to the code. Log in Claude Code on the homelab once with `claude auth login`.

**Per-handoff:** have the tool compute the needed secret-file set, compare hashes with the destination, and send only the missing or changed files, as an age-encrypted stream to the homelab's key over Tailscale SSH. Values never touch git, logs, or the agent's context. Transcripts get gitleaks-scanned before transfer.

### Cited Findings
- Tailscale SSH needs no key distribution, supports scp/sftp, can require periodic or per-connection IdP re-auth (`checkPeriod: always`), and revokes in seconds. — [Tailscale SSH KB](https://tailscale.com/kb/1193/tailscale-ssh)
- SOPS can encrypt to an existing ssh-ed25519 public key. The homelab's SSH key can be a recipient with no new key material. — [SOPS age docs](https://getsops.io/docs/usage/identities/age/)
- Self-hosted Infisical and Phase both run on Docker Compose and are meant to be reached over a private network such as Tailscale. — [Infisical homelab blog](https://infisical.com/blog/self-hosting-infisical-homelab); [Phase docs](https://docs.phase.dev/)
- `op run` scoped to a service account works headless. 1Password Environments can be scoped to service accounts. — [1Password CLI](https://www.1password.dev/cli/secrets-environment-variables/)
- `claude setup-token` can't do Remote Control, so the homelab needs a `/login` credential for `claude rc`. — [Claude Code authentication](https://code.claude.com/docs/en/authentication)

### Inferences

**Ranking for (a) one-time homelab bootstrap with many projects' .env files**
1. **tar over Tailscale SSH**, driven by the discovery pipeline. Example: `tar -cz --null -T list | ssh homelab 'tar -xz -C ~/developer/<group>/<repo>'`. It's fast, needs no new infrastructure, the channel is already authenticated, and files keep 0600 permissions if you set umask or chmod. Risk: plaintext at rest on the homelab, the same as on the laptop.
2. **SOPS+age with laptop + homelab + offline backup recipients**, committing `*.enc.env` files. This gives encrypted history and per-machine revocation. It costs a per-repo `.sops.yaml` and changes the dev workflow to `sops exec-env` or decrypt-on-checkout.
3. **Self-hosted Infisical or Phase on the homelab.** It works well if you want a central UI and audit log. The trade-off is that the secret store and its database sit on the homelab, so a homelab compromise exposes everything. It also adds another service to keep running (Infisical needs 4 GB RAM per the guide).
4. **1Password `op run` / Environments.** Good if the user already pays for 1Password. A service-account token on the homelab is a single long-lived bearer secret, so scope it to dev vaults only.
5. **dotenvx.** Good DX, but it still needs `.env.keys` transferred and is per-file. Viable if repos already use it.
6. **git-crypt / transcrypt.** Not recommended: shared symmetric keys, so revoking a machine means rotating the key and re-encrypting.
7. **magic-wormhole / croc / Taildrop.** Unnecessary inside a tailnet. They're fine as a one-off manual fallback.

**Ranking for (b) the per-handoff building block in an open-source tool**
1. **Built-in "sync secret files" step over the existing Tailscale SSH channel:**
   - Discover files.
   - Hash each one (`sha256`) on both sides via `ssh homelab sha256sum …`.
   - Send only the diff through `age -r <homelab-recipient> | ssh homelab 'age -d -i key | tar x'`, or just rely on SSH encryption and write with 0600.
   - Never echo values. Print only file paths and counts.
   - Run it as a subprocess whose stdout and stderr never reach the agent: either the tool is invoked by the user rather than by the agent, or its output is limited to "3 files synced".
2. **Pluggable providers:** if a repo has `.sops.yaml`, `.env.keys`, or an `op://`-referencing `.env.tpl`, push nothing and just verify the destination can decrypt or resolve (the homelab has an age key, a `DOTENV_PRIVATE_KEY`, `OP_SERVICE_ACCOUNT_TOKEN`, or `PHASE_HOST`).
3. **Claude auth check:** verify, don't copy. If the homelab isn't logged in, stop and tell the user to run `claude auth login` in the homelab pane.
4. **Transcript gate:** run `gitleaks stdin --redact` or `trufflehog filesystem --no-verification` on the session JSONL before transfer, and block or redact on findings.
5. **Agent hardening on the destination:** write `.claude/settings.local.json` (or user settings) with:
   - `Read(.env)` / `Read(.env.*)` / `Read(**/*.pem)` deny rules
   - `sandbox.enabled`
   - `sandbox.credentials.files` deny for `~/.claude/.credentials.json`, `~/.ssh`, and `~/.config/sops/age/keys.txt`
   - `sandbox.credentials.envVars` deny for injected secret vars

**Threat model**
- **Compromised homelab:**
  - Every plaintext secret there, and any manager token, is exposed.
  - SOPS+age limits damage only if secrets stay encrypted at rest, but dev servers need plaintext at runtime, so in practice rotate everything that was on the host.
  - Self-hosting Infisical, Phase, or Vault on the homelab makes the homelab the crown jewels. A cloud manager (1Password) with a dev-only service account limits the blast radius to that vault.
  - Use dev or test credentials on the homelab, never production keys.
- **Leaked transcript:**
  - The main novel agent risk. Values the agent read or printed are in JSONL in clear text.
  - Mitigations: prevention (deny rules plus sandbox deny or mask), detection (gitleaks/trufflehog before transfer and before any share), and rotation of anything found.
  - Run TruffleHog verification (`--results=verified`) only locally and deliberately, since it contacts provider APIs.
- **Stolen laptop:**
  - FileVault plus the Keychain protect the Claude login.
  - Plaintext `.env` files and age `keys.txt` are exposed if the disk is unlocked.
  - Remove the laptop's Tailscale node (instant SSH revocation), remove its age recipient and run `sops updatekeys` plus rotate data keys, and revoke the Claude login (`/logout` on another device, or from account settings, unverified).
- **Key rotation:**
  - SOPS lets you re-key recipients without changing secret values, but revoking a reader that ever had plaintext requires rotating the actual upstream API keys.
  - The same holds for every approach: encryption controls access going forward, while rotation is the only remedy for past exposure.

### Gaps
- No benchmark data on "seconds" latency for each approach. The claim that hash-diff over Tailscale SSH is sub-second for a handful of small files is an inference.
- Didn't verify `claude auth status` as a subcommand name. The docs reference `claude auth login`, `claude auth logout`, and `/status`.
- Vault and OpenBao (self-hosted), Doppler, Teller, envchain, and pass/secret-service weren't researched with primary sources this session. Any claims about them in the final report should be flagged as background knowledge.
