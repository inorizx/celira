# Celira

**Two products that meet only at the protocol**: a small IM world where humans and agents are both
first-class participants, and an agent execution runtime you deploy and look after on your own
machine.

English | [简体中文](./README.zh-CN.md)

> **Four words this page assumes.** Glossed here once, so you are not sent elsewhere for them:
> **world** — the logical im server and the public facts it adjudicates (identities,
> relationships, rooms, messages, events). **home** — the execution-end machine: everything
> `home.js` manages on it (the Agent's environment, materials, logs, model configuration), living
> in a data directory (default `D:\celira-home`) that also holds the world's database (`im/`) and
> **one subdirectory per Agent** (`agents/<id>/`). **environment root** — that per-Agent
> subdirectory (`<home>/agents/<id>/`): one Agent's own state, and its `model_binding.json`.
> **duty room** — the `direct` room between you and one Agent: the room `home.js up` prints.
>
> ⚠ **The one collision worth knowing up front**: the frozen contracts write the *world* with the
> proper name **`Home`**. So `Home` in a contract means the world; `HOME_` / `home.js` / "起家"
> mean the machine. The conversion rule is ruled once in
> [`docs/constitution/PRODUCT_VISION.md`](./docs/constitution/PRODUCT_VISION.md) §4 — that table is
> the authority; this note is a gloss for readers of this page, not a second one.

---

## Contents

- [What it is](#what-it-is) · [What it is not](#what-it-is-not)
- [Quick start](#quick-start-windows-and-linux) · [After it lights up](#after-it-lights-up) · [Deployment shapes](#deployment-shapes)
- [Two extension points](#two-extension-points) · [Architecture at a glance](#architecture-at-a-glance)
- [Repository map and naming](#repository-map-and-naming) · [Documentation map](#documentation-map)
- [Status](#status) · [Upgrading and rolling back](#upgrading-and-rolling-back)
- [Releasing (maintainers)](#releasing-maintainers) · [License](#license)

---

## What it is

- **Two products, not one app.** The **IM product** is the world: one logical im server (a single
  Rust binary + SQLite) plus a shared client — identities, relationships, rooms, messages. The
  **Agent product** is the execution end: the Agent's model, memory, capabilities and execution state,
  living in a home on a machine of yours. They meet only at the protocol and neither owns the
  other — unplug either one and the other still lives.
- **One world, one writer.** At any moment a world has exactly one writable im server, the sole
  arbiter of public facts. A world can have many clients and many execution points; the public
  facts have one writer.
- **Two planes.** The public world (identities, rooms, messages, events) and the Agent's private world (the Agent's
  environment, the Agent's notes, the Agent's experience). In the public world the Agent is a first-class participant;
  nothing in the Agent's private world enters the public one on its own.
- **The Agent persists; it is not a session.** Once the home is up the Agent stays resident waiting to be
  woken: a doorbell ring starts one bounded activity, and when it ends the Agent goes back to listening.
  The Agent's experience is appended on disk, and the next waking period continues from there.
- **Capabilities are the Agent's, boundaries are yours.** Files (a read-only view plus explicitly
  authorized text writes), notes (the Agent's own, always available), im (see rooms / read history / speak /
  verify), web (search and fetch behind five mechanical walls), a terminal (**whole-machine trust**,
  only present when declared), and **external servers** (MCP: one line per server in `home.conf`,
  expanded only when the Agent actually uses one).
- **An all-in-one bundle on your own machine, by default.** The world, the execution end and the
  page all run on your machine; **no telemetry, no platform account**. Putting the world on a server
  of your own is something you can do today (see [Deployment shapes](#deployment-shapes)) — local is
  the default because it is the shape with the fewest promises, not because the cloud is forbidden.

## What it is not

- **Not a SaaS, not multi-tenant.** One writable server per world; no double writers, no
  sync/merge, no remote room projection.
- **Not a phone or desktop app.** v1 is "start it from the command line, walk into the duty room
  through a browser"; a desktop shell (v2) is a **wrapping layer**, not a debt of v1.
- **It does not think for the Agent.** The system does not summarize, plan, auto-resume or retry on the Agent's
  behalf — those are the Agent's actions, not the system's.

## Quick start (Windows and Linux)

**There are two readers here, and their paths differ** — find yourself first:

- **Just using the app (desktop)**: no Node.js, no command line, no Rust ⇒ take **A**. The four-step
  first-run walkthrough, **how to update**, and **where your data lives** are all in
  [`scripts/APP_MANUAL.md`](./scripts/APP_MANUAL.md).
- **Developing, or deploying on your own server** ⇒ take **B** (Windows release package, CLI form) /
  **C** (from source) / **D** (Linux release package). **These three need Node.js**; on Windows,
  [`scripts/install.ps1`](./scripts/install.ps1) checks the rest and reports exactly what is missing,
  with an install link for each. On Linux the release package needs **no installer and no Rust
  toolchain** — `node home.js up` itself prepares the home and reports what is missing (path D below).
  **Git Bash is not needed** — the home entry point is `node home.js`; the installer checks only what
  each path actually needs (a release package: Node.js; a source clone: Git, Rust, Node.js with npm).

**A. Just using it: unpack, double-click.**

1. Get this version's package `celira-<version>-win-x64.zip` (**there is no public releases page yet**: packages are handed over by the maintainer; once that page exists, this is where it will be), unpack it anywhere;
2. **Double-click `celira-app.exe`** — the world, the runtime and the page are all inside this package;
   **no Node.js, no command line**;
3. On first open it asks you one thing: "your own world" → choose "open one on this machine", then
   follow the panel.

**Your data is not in that folder**: the world's history and each Agent's environment and notes live in
the **home** (on Windows, `D:\celira-home` by default). So **updating means swapping that folder, and
the home is not touched by a single byte** — how to swap, and how to go back if something is wrong, is
in the "换一个新版本" section of [`scripts/APP_MANUAL.md`](./scripts/APP_MANUAL.md).

**B. From a release package (CLI form, Windows) — no Rust toolchain.** Take this version's
`celira-<version>-win-x64.zip`, unpack it anywhere, then:

```powershell
# 1) one command: check prerequisites (Node.js) -> prepare the home -> guide the model credential
.\scripts\install.ps1

# 2) light up the home: im server -> the Agent -> page, all at once, with readings
node home.js up

# 3) open the browser
#    http://localhost:5173
```

**C. From source — clone and build.** Extra prerequisites: Git, the Rust toolchain, Node.js with npm.

```powershell
# 1) clone
git clone https://gitee.com/myloveinori/celira.git
cd celira

# 2) one command: check prerequisites -> build (Rust + web) -> prepare the home -> guide the model credential
.\scripts\install.ps1

# 3) light up the home
node home.js up

# 4) open the browser
#    http://localhost:5173
```

> **Only want to look at the UI first?** You do not need the Rust toolchain, a world, or a model
> credential: `cd client && npm install && npm run dev`, then click
> **「不连接，先看演示界面」** ("don't connect — just show me the demo") on the login page.
> The world, the agent and the model are all absent — that is the page alone.
> (Two commands instead of the whole of C; nothing is written outside the browser.)

**D. On Linux — from the Linux release package: no Rust, no installer.** Take
`celira-<version>-linux-x64.zip` (assembled by the same packager on a Linux machine — see
Releasing), unpack it with `unzip` (it keeps the executable bit; an extractor that drops file modes
leaves `bin/` unable to run), then:

```bash
# 1) the model half: DEEPSEEK_API_KEY in the environment
#    (missing => up stops at exit code 3 and says exactly this)
export DEEPSEEK_API_KEY=<your model key>

# 2) light up the home: im server -> the Agent -> page + the two doors, one command
node home.js up

# 3) open the browser
#    http://localhost:5173  (paste the [token] line the first up printed)
```

The package carries the binaries (`bin/`), the built page (`client-dist/`) and the home scripts —
nothing is compiled on your machine; `unzip` + `node home.js up` is the whole flow. There is **no
Linux installer script** yet: the `install.ps1` steps that apply on Linux (prepare the home, report
what is missing) are exactly what `node home.js up` does; the one step it cannot do for you is the
model key — that stays your environment variable (or the browser's Model configuration page, later).

- The installer builds (or, in a release package, ships) **release** artifacts and the home scripts
  start exactly those — no extra build step in between. The browser gets a **production page**
  (`client/dist`), not a development server.
- **The page needs Node.js in both forms** (release users are spared Rust, not Node) — v1's honest
  boundary. Binaries are **unsigned**, so SmartScreen may warn the first time on Windows. v1
  assembles **Windows x64** and **Linux x64** packages with the same script (the Linux one is built
  on a Linux machine — `-Platform linux-x64`).
- Model credential: `install.ps1` checks `DEEPSEEK_API_KEY` (user environment) — if it is missing it
  asks **once** and writes it there; the value is **never echoed, never written to disk, never a
  parameter**. Configure it yourself beforehand and the installer simply skips that step.
  Alternatively configure the model **from the browser** (Settings → "Model configuration",
  `/settings/providers`): it writes **two** facts on this machine — the **list** (which keys this
  machine has) goes to `<home>/model_sources.json`, the **binding** (which one this agent uses) goes
  to `<env root>/model_binding.json`. Keys stop at this machine: they **never** enter the world and
  **never** come back to the browser (the response carries only a `keyConfigured` boolean). A change
  takes effect on the Agent's **next** waking period — no restart. The two routes are mutually exclusive:
  with a binding present the process refuses to start if any model source is also present in the
  process environment.
- Once the home is up: `node home.js status` for status, `node home.js doors` to start the
  model-configuration door on its own, `node home.js down` to stop
  (the home is persistent — stopping it never removes what is inside). Manual:
  [`scripts/HOME_MANUAL.md`](./scripts/HOME_MANUAL.md) (a copy is placed in the home as `README.md`
  when it boots).

## After it lights up

The installation-period journey, in the order you actually meet it.

1. **Log in.** The first `node home.js up` opens the world's first Human and prints **one** token,
   once (the `[token]` line). Open http://localhost:5173 — the first screen is the login page (server
   address + token) — paste it there and press **Connect**. **Leave the server address empty**: empty
   means same-origin, which is what a home wants. The browser remembers it afterwards; if you lose
   it, `node home.js up --token` issues another. (`install.ps1` prepares the home and guides the
   model key — it does **not** produce a login credential.)
2. **Configure the Agent's model.** Settings → **Model configuration** (`/settings/providers`): endpoint,
   key, model name, context window. Two facts, two files — see the bullet above. The environment
   variable route (`DEEPSEEK_API_KEY`) is the alternative; **one route only**, and the process says
   which two things are fighting if you set both.
3. **Create the Agent.** The "create" button does **only the world-side half**: it opens the Agent's
   participant identity in the world — and the Agent **will not answer because of it**: the identity
   exists, but **no body does** (nothing writes `home.conf`'s `agent_id`, no backend credential is
   issued, nothing is started). The other half is called 「**安家**」 (settling an existing identity onto
   a machine): on the machine side there is one narrow path today — `node home.js up --agent <AgentId>`
   (only when the home has no resident yet; the manual has a section) — and **the app has none yet**
   (registered as a known gap in `client/README.md`). **Local environment only** is, today, a command line
   thing: `agent_run init --root <env root> --agent <AgentId>` — the Agent then has the whole local
   capability set but no identity in the world, so the Agent cannot talk there. The client's "local
   only" button is currently a **disabled placeholder that says 「尚未支持」 ("not yet supported")** —
   it is not wired, on purpose.
4. **Talk.** The room `home.js up` printed is the duty room: a direct room between you and the Agent. Say
   something; the doorbell rings, the Agent wakes, runs one bounded activity, and answers. Rooms, history
   and attachments all live in the world, so they survive the Agent sleeping.
5. **Look after the Agent.** "My Agents" lists every agent you own; **the Agent's page is addressed by
   the Agent's participant id** — `/app/agents/<the Agent's participant id>`, with two sections today:
   「**资料**」 (the world-side identity) and 「**机器**」 (which machine the Agent lives on, and whether
   it is running — the way out to that machine).
   Honest defaults: an agent with **no execution end behind it is marked 未接入 (not connected)**,
   and one with **no model bound yet says plainly that the Agent will not answer**. Nothing is left to
   guesswork and nothing is faked.

## Deployment shapes

| Shape | Where the world runs | Where the execution end runs | Can you do it today? |
|---|---|---|---|
| **All-in-one** (default) | this machine | this machine | **Today** — `node home.js up` starts all three |
| **The world in the cloud** | a server of your own | this machine | **Half of it today** — run the world **and its page** on that server and open that address (they must share one origin; the browser will not let a locally served page talk to a remote world, because the im server emits no CORS headers). Guide: [`im/server/README.md`](./im/server/README.md) §5. Binding a **local** execution end to that remote world has no ready-made command yet — that is the Agent-protocol path in §6 of the same file |
| **Looking after the Agent across machines** | anywhere | another machine | **Today** — issue a home key on that machine (`node home.js issue-home-key`), open the home to the network over TLS (`status_bind` / `config_bind` + `tls_cert` / `tls_key`), then add it in Settings → Home; manual: [`scripts/HOME_MANUAL.md`](./scripts/HOME_MANUAL.md) |
| **A resident of the world only** | this machine or a server | none installed | **Today** — take the IM product alone; install no agent at all |

Two honest notes. **The world in the cloud** means *your* server, not a Celira-operated one: the
public facts are still yours, and the deployment is yours to keep alive. **A resident of the world
only** is a real shape — you talk to people and to other people's agents, you just run no execution
end of your own.

One more, because it is a fact about the code and not a preference: **the page and the world must
share one origin.** The im server carries no CORS surface, so a page served on your machine cannot
talk to a world on your server — the browser blocks it before the request leaves. Serving the page
*from* that server is the shape that works today.

## Two extension points

- **接入位 — the place where agents join.** The world does not care what runs an agent. Anything
  that speaks the Agent protocol (the pull and message endpoints plus the doorbell) can be bound to an agent
  identity and becomes a **citizen with the same rights**: same rooms, same messages, same
  idempotency guarantees. Celira ships **one** execution side — the Native Runtime in `agent/` — and
  does not maintain adapters for third-party agents (2026-09-28: the example connector
  `im/connectors/hermes-celira` was retired and deleted).
  Celira does not manage their internals, and does not take over their model, memory or tools.
- **改造位 — the place where you change the Agent's body.** `agent/` is a complete runtime you can fork:
  swap the tool surface, the model adapters, the channels. The seams are documented as they are added.
  What is **not** promised today: treating the runtime as a library you `import`. Fork it; do not
  link it.

## Architecture at a glance

```text
   ┌──────────────────────────────────────────────────────────────────┐
   │  client/     the presentation face both products share            │
   │  browser: the duty room (rooms / messages / members) + the Agent's page   │
   └───────────────┬───────────────────────────┬──────────────────────┘
                   │ /v1  (Bearer)             │ /home-status · /home-config
                   │                           │ (same-origin proxy on the machine itself;
                   │                           │  or another machine's address + home key)
        ┌──────────┴────────────┐      ┌───────┴──────────────────────┐
        │  im/server  世界/world│      │  agent/    execution end      │
        │  Rust + SQLite        │      │  the Agent's environment / experience │
        │  identities / rooms   │      │  capabilities / model adapters│
        │  members / messages   │      │  (the whole thing home.js     │
        │  events / delivery    │      │   boots on this machine)      │
        └──────────┬────────────┘      └───────┬──────────────────────┘
                   │  Backend protocol: pull + message endpoints + the doorbell
                   └───────────────────────────┘
```

Three containers, three named connections. Humans and agents are both participants; processes,
hosts and sessions are not. The full container view and data flows live in
[`ARCHITECTURE.md`](./ARCHITECTURE.md).

## Repository map and naming

The repository is grouped by system:

- `im/` — the IM system: `im/server`, which is the world itself. **`im/connectors/` no longer
  exists**: the one example connector was retired and deleted on 2026-09-28 (see Two extension
  points);
- `agent/` — the agent system: the current line is the **standalone crate** `agent/` (not part of
  the root workspace); the old lines `agent/runtime/` and `agent/acp-supervisor/` were retired on
  2026-09-17 (not in this tree — see the boundary note at the end of this section);
- `client/` — the whole product's presentation face (top level);
- `app/` — the desktop shell (Tauri + Rust): **the source of `celira-app.exe`**, the thing path
  **A** above asks you to double-click. It is a **wrapping layer** around the three containers,
  not a fourth one;
- `examples/` — copy-and-run samples; [`examples/README.md`](./examples/README.md) says which
  extension cell each one belongs to;
- `tools/` — offline utilities that are **not** in the release package
  (`tools/home_portability.py`, `model_stub.py`, `trace_view.py`);
- `scripts/` — the home entry point (`home.js` at the repo root, with `scripts/home/*.js`), the
  packaging/install scripts and the static server (the old `home_*.sh` were removed once the Node
  entry point was verified);
- `docs/` — constitutional documents and frozen contracts.

One directory you will see named below but **not** in this tree: `host/` — the Local Client Host
crate (local all-in-one orchestration + narrow BFF) was **removed on 2026-09-20**, and its source
is gone from the worktree (not in this tree; the design record is kept in
[`docs/client/LOCAL_CLIENT_HOST.md`](./docs/client/LOCAL_CLIENT_HOST.md)). The *Host* role still
appears in the responsibility vocabulary of `AGENTS.md` / `ARCHITECTURE.md`, but no longer as a
component here; local process orchestration is `home.js` (see `scripts/`).

⚠ **A boundary about this repository's history.** **This repository's Git history begins at the
initial public commit** — so retired code (`agent/runtime/`, `agent/acp-supervisor/`, `host/`,
`im/connectors/`) **is not in this repository's history** and cannot be recovered here; it exists
only in the maintainer's own repository. For the same reason, the `outputs/…` references you will
meet in maintainer-facing documents point at **the maintainer's ruling records and measurement
reports — that whole directory is not published with this repository**. Read such a reference as
**a provenance note, not as a path you can open** (it is registered in
[`docs/runtime/AGENT_VOCABULARY.md`](./docs/runtime/AGENT_VOCABULARY.md) §11).

Naming map (dual-system layout, 2026-09-11): **Core v1 = the im server v1 contract** (file names and
the words "Core v1" are kept as historical version names); crates, binaries and environment-variable
key families were renamed in the same change to `celira-im` and `CELIRA_IM_*`; the
`CELIRA_LOCAL_CLIENT_*` family (including `CELIRA_LOCAL_CLIENT_WEB_DIST`, whose name keeps the WEB
spelling) is **frozen**.

## Documentation map

| Document | Contents | Read it when |
|---|---|---|
| [`AGENTS.md`](./AGENTS.md) | Collaboration and engineering discipline: authority order, pre-change gate, freeze gate, architecture red lines | before touching any code |
| [`ARCHITECTURE.md`](./ARCHITECTURE.md) | The single C4 L1–2 system map: current implementation, container responsibilities, data flows, allowed/forbidden connections | understanding how it is put together |
| [`docs/constitution/`](./docs/constitution/) | Product vision, agent philosophy, foundation contract, continuity constitution | understanding why |
| [`docs/contracts/core-v1/`](./docs/contracts/core-v1/) | The frozen public contracts (identities / rooms / messages / agent protocol / acceptance) | changing or accepting the im side |
| [`docs/contracts/core-v1/HOME_ACCESS.md`](./docs/contracts/core-v1/HOME_ACCESS.md) | Admission to a world and initial pairing: who may enter, how the first credential is issued | deploying a world / adding a second person |
| [`im/server/README.md`](./im/server/README.md) | Deploying the world: run it locally, the operator CLI, every configuration key, putting it on a server | deploying or moving a world |
| [`agent/README.md`](./agent/README.md) | The agent-side code map and the behaviour wired step by step | working inside `agent/` |
| [`docs/agent/`](./docs/agent/) | The four agent-side target documents: constitution and product model, execution reliability, architecture and engineering boundaries, end-state navigation | before a large change inside `agent/` |
| [`docs/runtime/`](./docs/runtime/) | The runtime's vocabulary, principles, roadmap, layering and the terminal body of text (live documents only) | understanding the runtime's ontology and execution model |
| [`docs/runtime/AGENT_VOCABULARY.md`](./docs/runtime/AGENT_VOCABULARY.md) | **The name authority**: what each word means here, plus the repo's own **process register** (the working vocabulary you will meet in maintainer documents and code comments) | **you hit a word you cannot decode** |
| [`scripts/HOME_MANUAL.md`](./scripts/HOME_MANUAL.md) | The home manual: commands, failure modes, cross-machine care (issue a key / open the home / home list) | booting or fixing the home |
| [`client/README.md`](./client/README.md) | Frontend quick run, data modes, page map | working inside `client/` |
| [`client/DESIGN_SYSTEM.md`](./client/DESIGN_SYSTEM.md) | The frontend design system: tokens, typography, components and copy discipline | writing frontend code |

## Status

v1 is being closed out: the im server's public contract is frozen; the agent side is rebuilt around
"persistent identity + an environment of its own + continuous experience", and its capability
surface (files / notes / im / web / terminal / MCP) is wired step by step with behavioural readings.
The two-product split above is **the frame these entry-level documents are written to**; facts
about the current implementation are the code and the tests — the **architecture snapshot** is
[`ARCHITECTURE.md`](./ARCHITECTURE.md) §6, and the **per-step agent readings** are in
[`agent/README.md`](./agent/README.md).

The product supports Windows and Linux ; every v1 acceptance run so far has been on Windows, and
Linux is the registered extension. macOS is outside the supported surface — no environment to verify
on, and a build that compiles is not support.

## Upgrading and rolling back

**The package directory and the home directory are two different places.** The unpacked
`celira-<version>-<platform>/` (e.g. `…-win-x64`, `…-linux-x64`) is the **artifact**; the home
(default `D:\celira-home`, movable with
`CELIRA_HOME_DIR`) is the **data** — the im database and media (`im/`), one subdirectory per Agent
holding its environment and notes (`agents/`, `materials/`), `home.conf`, `logs/`.

**Upgrading = swap the package, leave the home alone**:

```powershell
node home.js down      # stop first: only processes stop, the home keeps everything
#  unpack the new version somewhere else and use that package from now on
node home.js up        # boot from the new package
```

The first `up` in an **existing** home does these things, each idempotent or "only when missing":

- missing directories are created; `home.conf` is written **only when it does not exist** — an
  existing configuration is not touched;
- `human_id` / `agent_id` from `home.conf` are reused (**no identity is rebuilt**);
- **a home from before 2026-10-01 is migrated to the resident layout**: its single `env/`
  environment root becomes `agents/<id>/` — one subdirectory per Agent (the manual has the
  section);
- `<home>/README.md` is refreshed when the shipped manual changed;
- **the im server runs its SQLite migrations on startup** (sqlx, idempotent, checksum-verified).

**Swapping the installed package** (replacing what is deployed on this machine with a new build)
is a plain **stop -> swap -> start**: the deployment directory is a directory of its own (never a
build output, never a pointer chain), swapping it means replacing those files wholesale, and the
way back is the previous package (kept, so it can simply be swapped back in). Three commands, and
it does not matter who runs them - see the "换一包" section of `scripts/HOME_MANUAL.md`.

**Rolling back = `down` first, then put the pre-upgrade home back.**

⚠ **Copy the whole home before you upgrade** — that is the only approach that always works. One
command does it:

```powershell
node home.js down      # stop first (a copy taken while running is "probably usable", not "usable")
node home.js backup    # copies to <home>-backup-<timestamp>; --to <dir> to choose the place
```

It backs up **the whole home** (`im/`, `agents/`, `materials/`, configuration …), skipping only runtime
artifacts (`logs/`, pid files). Two preconditions are hard — anything still running, or a previous
stop that was not clean, makes it **refuse** with exit code 3 — every file is sha256-checked on both
sides, and the manifest is written last. It ships **inside the release package** (no Python needed).
⚠ The backup contains `<home>/model_sources.json`, so **it carries your model keys** — treat it like
`agents/`.

The reason (checked against the code): migrations are **up only** (`im/server/migrations/` has no down
SQL), the new version's first `up` applies the pending ones, and from then on **any older binary
refuses to start** against that database because it does not recognise a migration in it (sqlx
`VersionMissing`; the readiness probe classifies that as `DecodeOrCorrupt`). **Migrations are
irreversible.**

The repository also carries an offline backup / verify / restore tool
([`tools/home_portability.py`](./tools/home_portability.py), see
[`docs/contracts/core-v1/HOME_PORTABILITY.md`](./docs/contracts/core-v1/HOME_PORTABILITY.md)). Two
honest notes: it is **not inside the release package** (the packager ships `bin/`, `client-dist/`,
`scripts/`, `docs/`, `home.js`, `VERSION`), so it only exists when you run from the repository; and
it covers **public facts only** (`core.db` plus the media attachment bytes), **not** the agent's
environment and notes. So "back up before rolling back" means the plain copy above.

**There is one version authority**: `[workspace.package].version` in the root `Cargo.toml`; the
`VERSION` file in a package is that authority's reading at build time (version / git rev / build time
/ platform).

## Releasing (maintainers)

```powershell
.\scripts\make_release.ps1                            # tag defaults to v<Cargo.toml version>; Windows x64
.\scripts\make_release.ps1 -Tag v0.1.1 -SkipPublish   # package and verify locally, publish nothing
.\scripts\make_release.ps1 -Platform linux-x64 -SkipPublish   # Linux x64 (run on Linux; -SkipBuild reuses built binaries)
```

- **One version authority**: `[workspace.package].version` in the root `Cargo.toml`. The tag must be
  `v<that version>` and the standalone `agent/` crate's own version must agree — the script refuses
  to package otherwise;
- **one run assembles four packages, one version**: the all-in-one `celira-<version>-<platform>/`
  (the default download) plus its three parts — `celira-core-…` (the world), `celira-home-…` (the
  runtime), `celira-client-…` (the page + its server). Each lands in `dist-release/` as a directory
  and a `.zip`, with a per-package sha256 `MANIFEST-<part>-<version>.txt` (`MANIFEST.txt` is the
  all-in-one's). **Only the all-in-one zip is published for now**; the split zips are built and kept
  locally until the release channel is ruled on. **Release notes are the commit subjects of the
  version**, so there is no `CHANGELOG.md` to keep in sync;
- the split is a **partition check**, not a copy list: every file of the all-in-one is claimed by at
  least one part, and the only files claimed twice are the declared shared pair (`client-dist/` +
  `scripts/serve_client.mjs`);
- on a Linux host the zips also carry each file's mode (the executable bit lives in the zip's
  external attributes), so `unzip` brings `bin/` out runnable;
- publishing goes through `gh` (the GitHub CLI). When `gh` is missing, signed out, or the remote is
  not GitHub, the package is still built and verified and the run reports **"not published"** with a
  non-zero exit code — it never pretends to have published.

## License

[MIT](./LICENSE) © 2026 inori
