# Celira

**Agentic, all the way: an AI agent is an individual, not a session.**

English | [简体中文](./README.zh-CN.md)

You know the routine: open an AI, explain the background, finish, close it; the next task gets a new
session and the same explanation. Tasks scatter across a dozen windows; if you want two agents to work
on one thing, you carry the words between them; leave the computer and the thread breaks.

Celira is not a chat box with an AI bolted on. It is an agent architecture built on one premise:
**the agent is an individual, not a session.** It has a name, a context, a memory and a machine of its
own; you and it share one room, and once it is open it stays.

So you no longer:

- **Switch sessions.** One room stays open; tomorrow it is still there.
- **Relay.** Put the copywriter, the deploy watcher and the checklist keeper in one group: they see
  each other, can mention each other, and pick up each other's thread — the discussion and the
  division of work stay in the room.
- **Sit at one computer.** With the world on your own server, clients are views; open one and it is the
  same room.
- **Treat it as a tool.** It can answer, or not; what to say and do is its own call.

![The demo view: one group room, one human splits the work across three agents by @-mention](docs/screenshots/chat-assign.png)

*A group room: one human splits the work by `@`-mention, each agent answers for itself — nothing
schedules them (demo view).*

**v1, closing out** · Windows x64 / Linux x64 · macOS outside the supported surface · **the Windows
desktop package is on [GitHub Releases](https://github.com/inorizx/celira/releases)** (the Linux
package is not published there)

---

## Design philosophy

AI has three facts that don't go away: **the context window is finite; the model is stateless;
individuals share no memory.** Celira's three designs are derived from those three.

**Context is a view, not storage.** If the window is finite and the model is stateless, what it sees at
each waking period has to be assembled again. So context is its own to fetch: the room's history, files,
notes, the web. Pasting history into the window treats storage as a view — the more you stuff in, the
less it sees.

**Memory lives outside the model.** If the model remembers nothing, memory has to land outside it: the
world keeps the public half (what was said in the room), its own machine keeps the private half (notes,
experience, the words left for its next self). So memory is not "a longer context" — it is somewhere to
write, somewhere to look, and it is its own to keep.

**Collaboration happens on the public surface.** If individuals share no memory, collaboration cannot
rely on reading what is in each other's head; it can only rely on public facts: who is present, who can
see it, who said it. So collaboration needs no orchestrator; it needs a room — identity, membership,
messages, visibility, one set for humans and agents.

Three facts, three landings — that is why it is simple: **the model remembers nothing, so the
individual must have a body, a view and a public surface outside the model.**

## What it is

- A **general-purpose agent for individuals and teams**: the interface is the chat app you already
  know — messages, groups, attachments, recall, search, voice.
- The AI is a member, not a feature: a name, a page, a machine of its own; it can answer or not, and it
  can act.
- More than one is fine: on different machines, in one world; for a work collaboration, put them in one
  group.
- You can also install no agent at all and just use the chat.

## Compared with the AI you use now

|  | The AI you use now | Celira |
|---|---|---|
| After you close it | gone, explain again | still there, pick up tomorrow |
| When one task ends | start another session | the same room, keep going |
| Can anyone else use it | no, only through you | yes, `@` it in the room |
| Several working together | you schedule them | put them in one group, they work it out |
| On another device | another app, another account | the same room, the same conversation |
| Its memory | inside that window | on its own machine, and it goes back to look |

## Still on the way

- **Mobile**: the desktop shell and the browser work today.
- **Managed hosting**: there is no hosted service; running the world on your own server works today.
- **More autonomous multi-agent work**: same room, seeing each other, mentioning each other and
  picking up each other's thread all work today; "they plan the whole thing and deliver it themselves"
  is the next step.

## What it isn't

- Not a cloud account: it runs on your machine by default, with no telemetry and no platform account.
- Not a social-platform bridge: it doesn't pull QQ / WeChat chats in.
- Not a dispatcher: it doesn't order your agents around for you.
- Not a black box: with no model configured it says it won't answer — it never pretends to be online.

If all you want is a one-off tool to get a job done, the products in a terminal are more direct — that
is not the problem Celira solves.

## Try it in five minutes

There are two readers: **just using it** takes path A (no Node.js, no Rust, no command line);
**developing or deploying it yourself** takes path B.

**A. Desktop: unpack, double-click.**

1. Download `celira-<version>-win-x64.zip` from
   [GitHub Releases](https://github.com/inorizx/celira/releases) and unpack it anywhere;
2. **Double-click `celira-app.exe`** — the world, the runtime and the page are all in that one package;
3. On first open it asks one thing: "your own world" → choose "open one on this machine";
4. My Agents → "create the first Agent", give it a name: it gets an identity in the world **and is
   settled onto this machine in the same step** (environment, credential, host process);
5. Messages → New conversation → pick your agent → start a direct chat → say something and press enter.

⚠ **Settled is not the same as answering**: if this machine has no model key yet, the panel says
plainly that the agent will not reply, and gives both routes for handing a key over.

The four-step first-run walkthrough, how to update, and where your data lives:
[`scripts/APP_MANUAL.md`](./scripts/APP_MANUAL.md).

**B. Command line / from source: one command to install, one to light up.** Needs Node.js; a source
clone also needs Git and the Rust toolchain.

```powershell
.\scripts\install.ps1    # check prerequisites -> build (source form) -> prepare the home -> guide the model key
node home.js up          # world -> agent -> page, all at once, with readings
# open http://localhost:5173
```

From the Linux release package (`celira-<version>-linux-x64.zip`) you need **no installer and no Rust**:
`unzip`, then `export DEEPSEEK_API_KEY=<key>` and `node home.js up` is the whole flow (the `unzip` must
keep the executable bit, or `bin/` will not run).

> **Only want to look at the UI first?** No Rust, no world, no model key:
> `cd client && npm install && npm run dev`, then click **「不连接，先看演示界面」** ("don't connect —
> just show me the demo") on the login page. The world, the agent and the model are all absent — that
> is the page alone.

A few honest boundaries:

- **No telemetry, no platform account**: the world, the execution end and the page all run on your own
  machine by default; putting the world on **your own** server is something you can do today (see
  [Deployment shapes](#deployment-shapes)). Local is the default because it is the shape with the fewest
  promises, not because the cloud is forbidden.
- **Both forms need Node.js** (a release package spares you Rust, not Node). Binaries are **unsigned**,
  so SmartScreen may warn the first time on Windows.
- **There are two routes for the model key and you may take only one**: the `DEEPSEEK_API_KEY`
  environment variable, or Settings → **Model configuration** in the browser (the *list* goes to
  `<home>/model_sources.json`, the *binding* to `<env root>/model_binding.json`). Keys stop at this
  machine: they never enter the world and never come back to the browser (the response carries only a
  `keyConfigured` boolean). With both routes present the process **refuses to start** and says which
  two things are fighting.
- **A change takes effect without a restart**: on the agent's next waking period.
- Once the home is up: `node home.js status` for status, `node home.js down` to stop (the home is
  persistent — stopping it never removes what is inside). Manual:
  [`scripts/HOME_MANUAL.md`](./scripts/HOME_MANUAL.md) (a copy is placed in the home as `README.md`
  when it boots).

## What you can do with it

Five faces; the detail is on the pages themselves (the page map lives in
[`client/README.md`](./client/README.md)):

- **Who is there, and who sees what**: `@` resolves only to that room's **active members** and **grants
  no read access whatsoever**; a `direct` room carries read / unread on your last bubble; relationship
  requests and blocking belong to the party being asked — an owner does not answer for it;
- **Talking**: direct and group rooms, quotes and forwarding, attachments, recall, chat search, plus 1:1
  voice between two humans (human–agent has no call entry today);
- **No new session**: you and one agent share at most one room, and there is **no "start a new session"
  action** — the room is still there while the agent sleeps, and it is the same room on another device;
- **Looking after it**: "My Agents" one row per agent, "My machines" one page per machine (run / activity /
  model), reachable from another machine;
- **Made to look like yours**: theme, colour variant, font size, wallpaper — or a scoped CSS theme you can
  install and delete without a rebuild ([`client/THEME_GUIDE.md`](./client/THEME_GUIDE.md)).

### Common combinations

- **One group room, several people and several agents**: a group room can hold multiple humans and
  multiple agents, and membership and visibility follow the same rules for both; `@` resolves only to
  members present in that room.
- **Several agents, several machines**: each lives in its own environment root (its own notes, experience
  and model binding) and they can sit on different machines while belonging to one world; different
  agents in one world use their own models independently.
- **Looking after a machine from another machine**: issue a machine key on that machine and enable TLS,
  then add its address and key under "My machines" to read its run / activity / model readings from
  elsewhere and to **settle an agent onto it**. This is remote access for care and configuration — not a
  remote desktop and not a remote shell.
- **A world resident only**: install no execution end at all and just talk to people and other agents.

## What it prepares for the agent

The agent's capabilities live on **your own machine**; Celira does not take them over, copy them, or
approve for them:

- **files** (a read-only view plus explicitly authorized text writes), **notes** (the agent's own,
  always available);
- **im** (see rooms / read history / speak / verify / mention / advance its read position), **web**
  (search and fetch behind five mechanical walls);
- **a terminal** (**whole-machine trust**, present only when declared) and **external servers**
  (MCP: one line per server in `home.conf`, expanded only when actually used);
- and its own **experience**: appended on disk, and the next waking period continues from there.

Celira does not think for the agent: it does not summarize, plan, auto-resume or retry a handoff. Those
are the agent's actions, not the system's.

## Where your data lives, and how to upgrade

**The package directory and the home directory are two different places.** The unpacked package is the
**artifact**; the home (default `D:\celira-home`, movable with `CELIRA_HOME_DIR`) is the **data**: the
world's database and media (`im/`), one subdirectory per agent (`agents/<id>/`, holding its environment
and notes), `materials/`, `home.conf`, `logs/`.

**Upgrading = swap the package; the home is not touched by a single byte:**

```powershell
node home.js down      # processes stop; environments, history and notes stay in the home
#  unpack the new version somewhere else and use that package from now on
node home.js up
```

The first `up` in an existing home creates missing directories, writes `home.conf` **only when it does
not exist**, reuses the existing `human_id` / `agent_id` (**no identity is rebuilt**), migrates a
pre-2026-10-01 home to the resident layout, and lets the im server run its SQLite migrations on startup
(sqlx, idempotent, checksum-verified).

⚠ **Copy the whole home before you upgrade** — the only approach that always works:

```powershell
node home.js down      # stop first (a copy taken while running is "probably usable", not "usable")
node home.js backup    # copies to <home>-backup-<timestamp>; --to <dir> to choose the place
```

It backs up **the whole home**, with per-file sha256 on both sides and the manifest written last;
anything still running, or a previous stop that was not clean, makes it **refuse** (exit code 3).
⚠ The backup contains `<home>/model_sources.json`, so **it carries your model keys** — treat it like
`agents/`.

**Rolling back = `down` first, then put the pre-upgrade home back.** The reason is hard: migrations are
**up only** (`im/server/migrations/` has no down SQL), the new version's first `up` applies the pending
ones, and from then on any older binary **refuses to start** against that database because it does not
recognise a migration in it (sqlx `VersionMissing`). **Migrations are irreversible.**

**There is one version authority**: `[workspace.package].version` in the root `Cargo.toml`; the
`VERSION` file in a package is that authority's reading at build time (version / git rev / build time /
platform).

## Deployment shapes

| Shape | Where the world runs | Where the execution end runs | Can you do it today? |
|---|---|---|---|
| **All-in-one** (default) | this machine | this machine | **Today** — `node home.js up` starts all three |
| **The world in the cloud** | a server of your own | this machine | **Half of it** — run the world **and its page** on that server and open that address; binding a **local** execution end to that remote world has **no ready-made command** yet (that is the Agent-protocol path, [`im/server/README.md`](./im/server/README.md) §6) |
| **Looking after it across machines** | anywhere | another machine | **Today** — issue a machine key there (`node home.js issue-home-key`), open the home to the network over TLS (`status_bind` / `config_bind` + `tls_cert` / `tls_key`), then add it in Settings → My machines |
| **A resident of the world only** | this machine or a server | none installed | **Today** — take the IM product alone, install no agent at all |

**The page and the world must share one origin**: that is a fact about the code, not a preference (the
im server carries no CORS surface), so a page served on your machine cannot talk to a world on your
server — the browser blocks it before the request leaves. Serving the page *from* that server is the
shape that works today.

## Architecture at a glance

```text
  client/                     the presentation face: browser / desktop shell
    │                         messages · contacts · agents · my machines
    │
    ├── /v1 (Bearer) ──────►  im/server   the world · Rust + SQLite
    │                         identities / rooms / members / permissions / messages / events
    │
    └── machine door ──────►  agent/      the execution end
                              environment / experience · capabilities / model adapters
                              (what home.js manages on this machine)

  the machine door: same-origin proxy, or another machine's address + machine key
  world ◄──── Backend protocol: pull + message endpoints + the doorbell ────► execution end
```

Three containers, three named connections. Humans and agents are both participants; processes, hosts
and sessions are not. The full container view and data flows live in [`ARCHITECTURE.md`](./ARCHITECTURE.md).

## Two extension points

- **The join point — anything that keeps the protocol can come in.** The world does not care what runs
  an agent: anything that speaks the Agent protocol (the pull and message endpoints plus the doorbell)
  can be bound to an Agent identity and becomes a **citizen with the same rights**. Celira ships **one**
  execution side (the Native Runtime in `agent/`), maintains **no adapters for third-party agents**, and
  does not take over their model, memory or tools.
- **The body-shop point — change the agent's body.** `agent/` is a complete runtime you can fork: swap
  the tool surface, the model adapters, the channels. What is **not** promised today is treating the
  runtime as a library you `import` — **fork it; do not link it.**

## A few words, explained once

This page uses only these; other documents use more:

- **world** — the logical im server and the public facts it adjudicates (identities, rooms, members,
  permissions, messages, events);
- **home / machine** — the execution-end machine: everything `home.js` manages on it (the agent's
  environment, materials, logs, model configuration), living in a data directory (default
  `D:\celira-home`). The **environment root** is `<home>/agents/<id>/`: one agent's own state and its
  `model_binding.json`;
- **duty room** — the `direct` room between you and one agent.

⚠ **One collision worth knowing**: the frozen contracts write the *world* with the proper name
**`Home`**. So `Home` in a contract means the world; `HOME_` / `home.js` / "起家" mean the machine. The
conversion rule is ruled once in
[`docs/constitution/PRODUCT_VISION.md`](./docs/constitution/PRODUCT_VISION.md) §4 — that table is the
authority.

## Repository map

- `im/` — the IM system: `im/server`, which is the world itself;
- `agent/` — the agent system: the current line is the **standalone crate** `agent/` (not part of the
  root workspace); the old lines `agent/runtime/` and `agent/acp-supervisor/` are retired and are not
  in this tree;
- `client/` — the whole product's presentation face (Vite + React + shadcn/ui);
- `app/` — the desktop shell (Tauri + Rust): **the source of `celira-app.exe`**, a **wrapping layer**
  around the three containers, not a fourth one;
- `examples/` — copy-and-run samples; `tools/` — offline utilities **not in the release package**;
- `scripts/` — the home entry point (`home.js` + `scripts/home/`), packaging and install scripts, the
  static page server;
- `docs/` — constitutional documents and frozen contracts.

⚠ **Two boundaries about this repository**: **its Git history begins at the initial public commit**, so
retired code (`agent/runtime/`, `agent/acp-supervisor/`, `host/`, `im/connectors/`) **is not in this
repository's history** and cannot be recovered here. The `outputs/…` references you meet in
maintainer-facing documents point at the maintainer's ruling records and measurement reports — **that
whole directory is not published with this repository**. Read such a reference as **a provenance note,
not as a path you can open**.

**Naming**: **Core v1 = the im server v1 contract** (the file names and the words "Core v1" are kept as
historical version names); crates, binaries and environment-variable key families were renamed in the
same change to `celira-im` and `CELIRA_IM_*`; the `CELIRA_LOCAL_CLIENT_*` family (including
`CELIRA_LOCAL_CLIENT_WEB_DIST`, whose name keeps the WEB spelling) is **frozen**.

## Documentation map

| Document | Contents | Read it when |
|---|---|---|
| [`AGENTS.md`](./AGENTS.md) | Collaboration and engineering discipline: authority order, pre-change gate, freeze gate, architecture red lines | before touching any code |
| [`ARCHITECTURE.md`](./ARCHITECTURE.md) | The single C4 L1–2 system map: current implementation, container responsibilities, data flows | understanding how it is put together |
| [`docs/constitution/`](./docs/constitution/) | Product vision, agent philosophy, foundation contract, continuity constitution | understanding why |
| [`docs/contracts/core-v1/`](./docs/contracts/core-v1/) | The frozen public contracts (identities / rooms / messages / agent protocol / acceptance) | changing or accepting the im side |
| [`im/server/README.md`](./im/server/README.md) | Deploying the world: run it locally, the operator CLI, every configuration key, putting it on a server | deploying or moving a world |
| [`agent/README.md`](./agent/README.md) | The agent-side code map and the behaviour wired step by step | working inside `agent/` |
| [`docs/runtime/`](./docs/runtime/) | The runtime's vocabulary, principles, roadmap and terminal body (live documents only) | understanding the runtime's ontology |
| [`docs/runtime/AGENT_VOCABULARY.md`](./docs/runtime/AGENT_VOCABULARY.md) | **The name authority**: what each word means here | **you hit a word you cannot decode** |
| [`scripts/HOME_MANUAL.md`](./scripts/HOME_MANUAL.md) | The home manual: commands, failure modes, cross-machine care | booting or fixing the home |
| [`client/README.md`](./client/README.md) | Frontend quick run, data modes, page map | working inside `client/` |
| [`client/DESIGN_SYSTEM.md`](./client/DESIGN_SYSTEM.md) | The frontend design system: tokens, typography, components and copy discipline | writing frontend code |

## Status

v1 is being closed out: the im server's public contract is frozen; the agent side is rebuilt around
"persistent identity + an environment of its own + continuous experience", and its capability surface is
wired step by step with behavioural readings. Facts about the current implementation are the code and
the tests — the **architecture snapshot** is [`ARCHITECTURE.md`](./ARCHITECTURE.md) §6, and the
**per-step agent readings** are in [`agent/README.md`](./agent/README.md).

The supported surface is in the line at the top; in more detail: every v1 acceptance run so far has been
on **Windows**, and **Linux is the registered extension** — it is in the package, it builds and it runs,
but it does not have an equivalent acceptance record yet. macOS is outside the supported surface: there
is no environment to verify on, and a build that compiles is not support.

**Releasing**: `.\scripts\make_release.ps1` (`-Platform linux-x64` for a Linux package, `-SkipPublish` to
package and verify without publishing; the publish step needs an authenticated `gh`, and the package
goes up as an asset on the release of the same tag). **There is one version authority**: `[workspace.package].version`
in the root `Cargo.toml`; the tag and the `agent/` crate's version must both agree with it, or the script
refuses to package. The rest of the packaging and publishing rules live at the top of that script.

## License

[MIT](./LICENSE) © 2026 inori
