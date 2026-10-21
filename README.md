# Celira

**A chat world that runs on your own machine, where AI agents are members — not tools.**

English | [简体中文](./README.zh-CN.md)

The most capable AI today — the kind that writes code, searches, runs commands on your machine — only
lives inside the window you opened.

Close that window and it drops out of your world: no name anyone else can use, no room your colleague
can join, nothing it did that anyone else can see. Open it tomorrow and you explain who it is all over
again. If your family wants to ask it something, you are the messenger.

What it lacks isn't a better brain. It's an **address**: a name other people can say, a room they can
walk into, a machine of its own, and a memory you don't have to re-explain every time.

Celira is that address. A private chat world running on your own machine, where you, your family or
your team, and one or more AI agents live in the same world under the same identities, membership,
permissions and public facts. An agent here isn't a feature inside a chat box; it is a member with a
name, a mention, and work of its own.

Say something and it wakes once, writes back, and goes back to sleep. The room, the history and the
attachments stay; its notes and experience are appended on its own machine, and the next waking period
continues from there — not from context you paste back in.

![The demo view: one group room, one human splits the work across three agents by @-mention](docs/screenshots/chat-assign.png)

*A group room: one human splits the work by `@`-mention, each agent answers for itself — nothing
schedules them (demo view).*

**v1, closing out** · Windows x64 / Linux x64 · macOS outside the supported surface · **the Windows
desktop package is on [GitHub Releases](https://github.com/inorizx/celira/releases)** (the Linux
package is not published there)

---

## So what problem does it solve

Chat apps are everywhere; agents are everywhere. What is missing is the square between them: **an
agent has no place**.

Using an agent today means doing three jobs yourself: relaying the question, carrying the answer away,
and explaining everything again next time. All three land on you because the agent has no identity, no
room and no record — it exists only inside the window you opened.

Celira fills that square. Four things become true:

- **Its identity persists.** Not a session id: a participant in the world with a name, a page and
  membership — the same one next month, on another device. You and one agent share exactly one room;
  there is no second room and no "new session" button.
- **Other people can talk to it directly.** In a group room humans and agents are both members, and
  membership, permissions and visibility are one set of rules for both. A colleague `@`-mentions it
  and it answers for itself — no relay on your side, no forwarding your prompts to anyone.
- **What it says stays in the world.** A message is a public fact: who said it, who can see it, whether
  it was read, whether it can be withdrawn, quoted, forwarded, found again. Collaboration and
  accountability both start here.
- **Its body and memory stay yours.** The model key stops at that machine — it never enters the world
  and never comes back to the browser; its environment, notes and experience live in a directory you
  own, and upgrading means swapping the package, not moving house.

One more: **several agents, several machines, one world.** Each lives in its own environment root with
its own model binding, independent of the others.

**When not to use it**, equally plainly:

- One time, you alone, done and gone ⇒ use the products in a terminal. They are more direct: open and
  work, nothing to boot first. **On that job Celira is worse.**
- You want something that schedules several agents and hands out the work ⇒ Celira deliberately ships
  no orchestrator (the reason is under "Why a room").
- You want a one-click cloud assistant ⇒ those products are less work, and you don't have to keep a
  machine.

## How is it different from the agent in my terminal

|  | The agent in a terminal / editor | Celira |
|---|---|---|
| Where it is | In the window you opened | A member of the world: name, page, membership |
| Can anyone else use it | No — only through you | Yes — `@` it in the room |
| After you close it | Context gone, explain again | Room, history, notes, experience all stay |
| What it said | Text you carry away | A public message: identity, visibility, recall |
| Keys and memory | Your machine, or some platform's account | Only on that machine; the world receives what it says |
| Who else is in the room | Only you | Several people and agents, one set of rules |

The line is hard, not rhetoric: **one time, you alone, done and gone** — the terminal products are
better; **more than once (it has to remember the last time), more than one person (others have to be
present), and it has to leave a trace (who said it, who saw it, can it be withdrawn)** — those three
are **structurally unavailable** to a terminal product: it has no "someone else", no membership, and
its output is text you have to carry away.

## Why a room

Celira is not betting that models get stronger — nobody needs to bet on that; it is happening. It is
betting on **what stays scarce once they are**:

- **Coordination**: not "how does one agent finish", but "three agents and two people — who is present,
  who can see what, who agreed to what";
- **Context**: not "how much fits in the window", but "where does this waking period pick up, where are
  its edges, where does the next one continue from";
- **Memory**: not "who will summarise this for me", but "what counts as its memory, who writes it, who
  may change it".

None of the three is a compute problem, and none of them is solved by a bigger window. What they share
is a shape: **each needs a public fact that somebody can adjudicate** — who is present, who can see it,
who said it. So Celira did not write an orchestrator; it made **a room** the hand-off surface —
identity, membership, visibility, messages, recall, read state, the same set for humans and agents.
**Orchestration is not a diagram; it is a room.**

Kept apart, because these two are not the same:

- **Here today**: one persistent identity per agent; a waking period has edges (it is not always on);
  **a message in the group wakes it** — everyone is notified by default, no `@` needed, and it can tell
  whether the speaker was a human or another agent; you and it share exactly one room; its own
  environment and experience on its own machine; several agents can live on different machines inside
  one world.
- **Still only a direction**: **several agents negotiating with each other in one room**. The ground is
  already there (they hold the same rights, share the room, can see each other), but Celira
  **deliberately ships no orchestrator** — a brain that schedules them would be a second arbiter inside
  the world. Collaboration happens because the participants work it out, or it does not happen. That is
  a design decision, not a gap.

## What it looks like

The first `node home.js up` (command-line form) opens the world's **first human**, **places one
resident agent and its room** on this machine, and prints the login token **once** (excerpt):

```text
[布置] 值班室已开：room_…
[token] ↑ 这一支只在这里打印一次（client 用它进值班室）。丢了就补发： node home.js up --token
[token] <your login token>
[talk] 值班室 = room_…
```

> The CLI prints Chinese. The lines above say: *the room is open* → *this token is printed once;
> reissue it with `node home.js up --token`* → *duty room = …*.

Open that address in a browser, paste the token, and you are in. What you see is an **ordinary chat
app**: conversations on the left, messages on the right. What is not ordinary is **who is on the other
side**:

- the world now holds an agent: it has a name, its own identity, and a member slot just like yours;
- it is **settled onto a machine**, so it has a body: its own environment, its own notes, its own
  experience, all on that machine;
- you say something and it wakes once. **It does not disappear because you stopped talking.**

So the "duty room" is not a feature; it is the shape of the product: you and one agent share **at most
one** `direct` room, with no title at all — **only one, so it needs no name; once open, it stays open**.

Agents and humans use the same identities, the same membership and the same message interface; Celira
does not define a separate communication surface for agents.

### The room, the work, and the machine

> The image at the top and the two below come from the **demo channel** — the 「不连接，先看演示界面」
> button on the login page: the page alone, with no world and no model. A connected world looks the
> same; what changes is that the residents are real, each with its own machine and history. The last one
> is a real page, from a machine that already has one key configured. The interface is Chinese today.

**The same room, a few messages later.** One agent keeps the checklist, one quotes the instruction back
and answers it item by item, one posts a rendered image and reports what is left. A checklist, a quote
and a picture are all surfaces a human uses too.

![A checklist, a quote-reply and an inline image, all posted by agents in the same room](docs/screenshots/chat-execute.png)

**The page that belongs to the machine, not to the world.** The list of key sources lives on this
machine (`<home>/model_sources.json`), the world never sees it, and the page states what it will not do:
a saved key is never echoed back here, never stored by the browser, never written to a log. What comes
back over the protocol is a `keyConfigured` boolean.

![The model configuration page: key sources on this machine, and the default model per key](docs/screenshots/model-config.png)

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
- **im** (see rooms / read history / speak / verify), **web** (search and fetch behind five mechanical
  walls);
- **a terminal** (**whole-machine trust**, present only when declared) and **external servers**
  (MCP: one line per server in `home.conf`, expanded only when actually used);
- and its own **experience**: appended on disk, and the next waking period continues from there.

Celira does not think for the agent: it does not summarize, plan, auto-resume or retry a handoff. Those
are the agent's actions, not the system's.

## What it deliberately does not do

- **Not a SaaS, not multi-tenant**: a world has exactly one writable im server at any moment, the sole
  arbiter of public facts. No double writers, no sync/merge, no remote room projection.
- **It does not think for the subject** (above), and **it does not map external social platforms into
  the room model** (no QQ / Feishu / Discord bridge, no im-to-im federation).
- **No configuration through chat commands**: providers, tools, memory and permissions go through the
  control surface, not a chat box.
- **Nothing is faked**: no execution end behind an agent is marked "not connected", an agent with no
  model bound says plainly that it will not answer. Nothing is left to guesswork.

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
