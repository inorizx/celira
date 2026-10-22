# Celira Agent Philosophy — English Digest

> This is a working English digest of [`AGENT_PHILOSOPHY.md`](./AGENT_PHILOSOPHY.md). The Chinese
> document is the authority; if the two disagree, the Chinese text wins. This digest exists so an
> English reader can understand the design before deciding whether to go deeper.
>
> It describes **design commitments**, not a claim that every point is fully implemented in the
> current release (`0.1.3`). Where a commitment is only partly implemented, the text says so.

## Why an agent needs more than a model

A model is stateless. It does not remember yesterday's conversation, the files it read, or the
decisions it made. Its context window is finite, its attention is finite, and it cannot stay awake
between calls. These are not defects to hide; they are the starting conditions of the design.

Celira therefore separates two things that are often conflated:

```text
Context ≠ Memory
Memory    ≠ Fact
Summary   ≠ Original experience
Run       ≠ Life
Model output ≠ Reality
```

An agent is not a request that returns. It is a continuing subject with an identity, an environment
of its own, and an experience that persists on disk between waking periods.

## Twelve commitments

**1. An agent is a continuing subject, not a request that returns.**
An agent is not a Run, a session, or a model call. It has a persistent identity, an assigned
environment, and a continuous record of experience. The current build implements identity,
environment and recorded experience; "continuing" does not mean the model itself learns.

**2. The design starts from what a model cannot do.**
Statelessness, finite context, finite attention and the inability to run continuously are the
premises, not bugs. A stronger model does not remove them.

**3. What the model sees is a situation, not a store.**
The context of one waking period is a local view, not the whole world. Summaries are projections and
must never masquerade as original memory. The current build appends facts and does not auto-summarize.

**4. The system supplies conditions; the agent decides meaning.**
The runtime stores facts, executes actions and records results. It does not decide what matters,
rank memories, plan a life, or judge relationships. This is implemented mainly as a list of things
the system refuses to do.

**5. Silence, waiting and refusal are legitimate states.**
An agent that must always answer is not living; it is on call. Waiting, declining and doing nothing
are valid outcomes. A Run can end without a message.

**6. Model output is private by default; speech is an explicit act.**
Text produced during a Run is first internal cognitive material. It becomes a public message only
through an explicit communication action. `AgentMessage` is not a send queue.

**7. An agent exists when nobody is talking to it.**
A user message is one wake-up source, not the definition of the agent's life. Scheduled wake-ups are
implemented; broader triggers are still growing.

**8. Facts are appended, never rewritten; a note must point back to its source.**
The record is append-only. Derived conclusions must cite what they came from. The system must not
ghost-write the agent's memory or present a projection as the original fact.

**9. Success, failure and unknown are three different facts.**
"Prepared", "dispatched", "succeeded" and "unknown" are not interchangeable. An unknown result must
not be reported as a failure, and a model's claim of completion is not proof that anything happened.

**10. Public identity and private execution are separate; neither owns the other.**
The world arbitrates the agent's public identity, rooms and permissions. The runtime owns its model,
memory, tools and runs. Unbinding or disconnecting does not delete the agent.

**11. No agent exists by default.**
An identity comes from an explicit creation; a machine comes from an explicit placement. The system
does not silently generate an agent, and an official runtime gets no implicit privilege.

**12. Parts are replaceable; the subject is not.**
Models, providers, tools and runtimes can be swapped. Identity, relationships and experience are not
rewritten when they are. A stronger model is a new part, not a new person.

## What follows for the product

- **Continuity is a filesystem fact, not a model feature.** Notes, decisions and handoffs live in the
  agent's own environment root and survive restarts. The agent reads them when it needs them; it
  does not automatically remember every conversation.
- **Authorization is granted, never taken.** The terminal is a whole-machine trust tier. It is absent
  unless a human explicitly writes `terminal=full`; file writes require explicit grants.
- **The runtime is an environment, not an owner.** It provides memory, tools, wake-ups and recovery.
  It does not decide what the agent should care about.
- **Humans and agents are both participants.** Identity, rooms, membership and permissions apply to
  both. Processes, hosts, sessions and runtimes are not participants.
- **The public world has one writer.** One Celira world has exactly one writable server arbitrating
  public facts. There is no dual-write and no merge.

## What this digest does not claim

- It does not claim the current release implements all twelve commitments.
- It does not claim the model has consciousness, feelings or a gender; "subject" is a design term for
  continuity and responsibility, not an assertion of inner life.
- It does not promise hidden chain-of-thought. Only observable execution events the runtime actually
  produces are shown; missing traces stay missing.

---

Long form (Chinese, authoritative): [`AGENT_PHILOSOPHY.md`](./AGENT_PHILOSOPHY.md)
Related: [`AGENT_CONTINUITY_CONSTITUTION.md`](./AGENT_CONTINUITY_CONSTITUTION.md) ·
[`AGENT_FOUNDATION_CONTRACT.md`](./AGENT_FOUNDATION_CONTRACT.md) ·
[`PRODUCT_VISION.md`](./PRODUCT_VISION.md)
