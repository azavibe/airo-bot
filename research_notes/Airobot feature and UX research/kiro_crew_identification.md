# Identifying "Kiro Crew" (the bot app the user remembers)

Research date: 2026-11-21.

Method note for the report writer: web search worked, but every page fetch failed. kiro.dev, dev.to, aws.amazon.com and blog.localstack.cloud are outside this session's allowed network. Everything below therefore comes from search-engine result summaries of the cited pages, not from reading the pages in full. Verbatim quotes are as the search tool returned them. Where a summary named several sources at once, I cite the most specific URL it listed and flag the attribution as approximate in the Gaps sections.

## Q1. Which products match or nearly match "Kiro Crew" / "Kiro" / "Kiri" / "Kroo" / "Crew" in the AI-agent and bot space?

### Takeaway
A real product called exactly **Kiro Crew** exists. It is AWS's open-source, persistent agent workspace from the Kiro team, and its headline feature is **Bots**, which became generally available on 2026-11-05. It is almost certainly what the user means. Nearby names are AWS's Kiro IDE, a Shopify app called "Kiro: Sales AI ChatBot Agent", CrewAI, Kroolo's "Kroo AI", and Kiru by Stratamos. I found no AI product named "Kiri".

### Cited Findings
**Exact match: Kiro Crew (AWS / Kiro team)**
- Kiro Crew has an official product page at kiro.dev/crew and its own docs section. It is described as an open-source AI agent workspace that runs on your own machine, locally or on a remote host, and keeps agents working across sessions — [Kiro Crew page](https://kiro.dev/crew/); [Crew docs](https://kiro.dev/docs/crew/)
- It runs on the Kiro CLI and talks to agents through the Agent Client Protocol (ACP), so it extends Kiro rather than replacing it — [Kiro Crew complete guide (aimadetools)](https://www.aimadetools.com/blog/kiro-crew-complete-guide/); [kirotutorial](https://kirotutorial.com/kiro-crew/)
- The official docs call it a personal agent that is "persistent, self-learning, and self-evolving" — [Crew docs](https://kiro.dev/docs/crew/); [AWS Builder: What is Kiro Crew?](https://builder.aws.com/content/3HWzCQeYfWjDxsyKVsmKzVPne7w/what-is-kiro-crew-a-hands-on-guide-to-ai-agents-that-learn-fix-and-scale-your-code)
- The GitHub repo kirodotdev/KiroCrew describes it as "A persistent workspace for development work that self-improves and continues beyond one session" and is licensed Apache 2.0 — [GitHub kirodotdev/KiroCrew](https://github.com/kirodotdev/kirocrew)
- There is an official "Introducing Kiro Crew" changelog entry (version 0.1) — [Kiro changelog: Introducing Kiro Crew](https://kiro.dev/changelog/crew/0-1/)
- **Bots** are a named feature with their own launch post and docs section. Each bot is a persistent agent with its own threads, memory and workspace — [Introducing bots in Kiro Crew](https://kiro.dev/blog/crew-bots/); [Bots docs](https://kiro.dev/docs/crew/bots/)
- Tech press covered the bots launch: "Kiro lets AI bots continuously build software autonomously" and "Kiro introduces bots to manage AI agents in software development" — [Techzine](https://www.techzine.eu/news/devops/145611/kiro-lets-ai-bots-continuously-build-software-autonomously/); [ITdaily](https://itdaily.com/news/software/kiro-bots-manage-ai-agents/)
- AWS made Kiro bots generally available in Kiro Crew on 2026-11-05 — [VentureBeat](https://venturebeat.com/technology/aws-launches-agentic-ai-kiro-bots-with-memory-and-support-for-recurring-development-jobs); [vinish.dev](https://vinish.dev/kiro-crew-bots-generally-available)
- Naming history: the agents were first called "crewmates", then "Mates", and are now "Bots". The official changelog says "Mates are now Bots", and older `.crewjam` and `.crewmate` export files still import — [Crew changelog](https://kiro.dev/changelog/crew/); [PR #20965: rename crewmate to mate](https://github.com/kirodotdev/KiroCrew/pull/20965); [kirocrew-cookbook PR #10: rename crewmates → bots](https://github.com/requix/kirocrew-cookbook/pull/10)
- A GitHub issue proposed renaming Mate/Crewmate to "Jam". I found no evidence it shipped — [Issue #26417](https://github.com/kirodotdev/KiroCrew/issues/26417)

**Same family: Kiro IDE / Kiro CLI (AWS)**
- Kiro is AWS's agentic IDE built around spec-driven development, steering files and agent hooks — [Introducing Kiro](https://kiro.dev/blog/introducing-kiro/); [SD Times](https://sdtimes.com/ai/amazon-launches-spec-driven-ai-ide-kiro/)
- A DEV post asks "We already have Kiro IDE, Kiro CLI, Kiro Web - What exactly Kiro Crew is?", which shows that Kiro Crew is a separate product in a family of Kiro surfaces — [DEV: ericpham03](https://dev.to/ericpham03/we-already-have-kiro-ide-kiro-cli-kiro-web-what-exactly-kiro-crew-is-5hca)

**Name collision: "Kiro: Sales AI ChatBot Agent" (Shopify app, unrelated to AWS)**
- A Shopify App Store listing named "Kiro : Sales AI ChatBot Agent" offers 24/7 AI-powered chat that answers customer questions about products, orders, shipping and returns — [Shopify App Store: Kiro](https://apps.shopify.com/kiro)

**"Crew" match: CrewAI**
- CrewAI is an open-source Python framework from crewAI, Inc. for orchestrating autonomous AI agents. It is built on two ideas: Crews, which are teams of role-playing agents, and Flows, which are event-driven workflows — [GitHub crewAIInc/crewAI](https://github.com/crewAIInc/crewAI)

**"Kroo" match: Kroolo / Kroo AI**
- Kroolo is a work-management platform whose built-in assistant is called "Kroo AI". It can create projects, tasks, documents and goals from natural-language prompts — [Kroolo](https://kroolo.com); [techreviewer: Kroolo](https://techreviewer.co/products/kroolo)
- Kroolo pitches "Meet your new AI Team", with pre-built agents or agents built from scratch — [Kroolo AI agent](https://corp.kroolo.com/ai/ai-agent); [Kroolo help: create AI agents from scratch](https://intercom.help/kroolo/en/articles/9983185-create-ai-agents-from-scratch)

**"Kiri" near-match: Kiru by Stratamos**
- Product Hunt lists "Kiru – Assistant with the goal to Integrate AI into Everything". It transcribes conversations, takes screenshots, updates Salesforce and triggers API actions — [Product Hunt: Kiru](https://www.producthunt.com/p/kiru-by-stratamos/kiru); [Kiru launches](https://www.producthunt.com/products/kiru-by-stratamos/launches)

**Generic "bot app" and "AI team" apps that surfaced (no name match)**
- "Agentic Bots" (iOS, developer Ivan Garcia O) lets users create custom AI agents such as Lawyer, Analyst or Chef bots, and it is free — [App Store: Agentic Bots](https://apps.apple.com/us/app/agentic-bots/id6753194867)
- "Agent Teams AI" is an open-source desktop app that coordinates teams of autonomous AI agents — [GitHub 777genius/agent-teams-ai README](https://raw.githubusercontent.com/777genius/agent-teams-ai/main/README.md)
- "Teamily AI: Human+AI Team" (iOS) and "TeamDay: Team up with AI Ally" (iOS) also exist in the "AI team" app space — [App Store: Teamily AI](https://apps.apple.com/app/6761445638); [App Store: TeamDay](https://apps.apple.com/app/id6504249106)

### Inferences
- The user's memory, "Kiro Crew", "one particular bot app", fits AWS Kiro Crew almost word for word. Since 2026-11-05 the product's main user-facing unit has literally been called a "Bot". Bots work through Slack, Discord and Telegram, and the product is a "crew" of agents.
- The "Kiri" doubt is most likely a spelling wobble on "Kiro". No Kiri-named AI product turned up.
- The crewmate → Mate → Bot renames mean people who used it before November 2026 may remember "crewmates" or "mates". That could also explain a fuzzy recollection of the name.

### Gaps
- No product named "Kiri", "Kirocrew" (other than the AWS repo), "KiroCrew AI", "Kiro Agents" or "Kairo" turned up in the AI-agent space. I did not run a separate search for "Kairo", so that remains unchecked.
- I found no Telegram, WhatsApp or Discord bot, and no mobile app, named "Kiro Crew" outside the AWS product. Kiro Crew itself connects to those chat platforms (see Q2).
- Pages could not be fetched, so the exact wording on kiro.dev is unverified beyond search summaries.

## Q2. What is Kiro Crew: core concept, UX, standout features, pricing, and what users like and dislike?

### Takeaway
Kiro Crew is a persistent, self-learning personal agent workspace. You talk to named **Bots**. Each bot owns one long-running thread, its own memory and a workspace. It turns goals into proposed milestones, runs recurring jobs and check-ins, delegates long work to background **workers**, and reports back through approval-style cards. You can reach it from a desktop app, a web dashboard, the CLI, or chat apps such as Slack, Discord and Telegram. The software is free under Apache 2.0, but model work spends Kiro credits. Users like the continuity. Critics point to the Trust-by-default permissions, credit burn, and early-release rough edges.

### Cited Findings
**Core concept and architecture**
- Crew is an open-source personal AI agent that runs locally or remotely on your own hardware and is built on Kiro CLI — [Crew docs](https://kiro.dev/docs/crew/); [AWS Builder: Getting Started with Kiro Crew](https://builder.aws.com/content/3HYByHPgtbAQqGjoelhwVLzem7j/getting-started-with-kiro-crew)
- Self-evolving skills: Crew can "Synthesize reusable skills from repeated patterns, then inspect, refine, or remove them as your work changes", and memory, lessons and skills stay visible and editable — [Crew docs](https://kiro.dev/docs/crew/)
- Scheduled and event-triggered runs and memory are listed as core features — [Kiro Crew page](https://kiro.dev/crew/); [kirotutorial](https://kirotutorial.com/kiro-crew/)

**Bots (the "bot app" part)**
- A bot is one persistent conversation with you, a continuity that a regular chat session can't offer. The bot delegates work to workers, which run in separate sessions and report their results back to the bot — [How bots work](https://kiro.dev/docs/crew/bots/how-bots-work/)
- The bot handles short tasks itself and delegates longer ones to workers. You can keep chatting with the bot while workers run in the background — [How bots work](https://kiro.dev/docs/crew/bots/how-bots-work/)
- The bot and its workers share the same memory and workspace, so workers can use context the bot has gathered and save what they learn. "A new bot starts with its own empty memory store" — [How bots work](https://kiro.dev/docs/crew/bots/how-bots-work/)
- Isolation: one bot cannot read another bot's memory, message another bot's thread, or control its workers. The docs warn that memory isolation "is not a filesystem permission boundary" — [How bots work](https://kiro.dev/docs/crew/bots/how-bots-work/)
- Memory is stored as plain Markdown files, and users can read, edit and delete entries (third-party write-up) — [agentencode.de](https://agentencode.de/en/news/kiro-crew-bots-persistent-ai-agents-memory-approvals-2026/)
- A conference-style deck describes a bot as "a single point of contact" with a name, a goal, its own memory and worker sessions — [Speaker Deck: Bots in Kiro Crew (One point of contact for ongoing work)](https://speakerdeck.com/inariku/kiro-crew-bots-en)
- The thread is where you define goals, review proposals and receive results — [Bots docs](https://kiro.dev/docs/crew/bots/)
- Goals → milestones: after you approve a goal, the bot researches it and proposes measurable milestones — [Bots docs](https://kiro.dev/docs/crew/bots/); [Crew 0.10 changelog: "Crewmate Goals and Dashboards for Everyone"](https://kiro.dev/changelog/crew/0-10/); [PR #19539: crewmate goals with milestones, check-ins and reports](https://github.com/kirodotdev/KiroCrew/pull/19539)
- Check-ins can be daily, weekly or monthly. They are off by default and require a goal. The weekly report summarizes completed work and progress toward milestones, and reports are saved as A4 documents in Artifacts — [Bots docs](https://kiro.dev/docs/crew/bots/); [Crew 0.10 changelog](https://kiro.dev/changelog/crew/0-10/)
- The bot presents goals, milestones, check-ins and reports as cards that you confirm — [Speaker Deck: Bots in Kiro Crew](https://speakerdeck.com/inariku/kiro-crew-bots-en); [Bots docs](https://kiro.dev/docs/crew/bots/)
- Recurring jobs can be set from the chat, for example reviewing overnight CI failures or drafting release notes each Friday. Bots oversee recurring responsibilities, coordinate other agents and remember the decisions developers make — [VentureBeat](https://venturebeat.com/technology/aws-launches-agentic-ai-kiro-bots-with-memory-and-support-for-recurring-development-jobs)
- Users can pick the AI model per bot and per worker agent, which can cut costs on simpler tasks — [Introducing bots in Kiro Crew](https://kiro.dev/blog/crew-bots/); [Techzine](https://www.techzine.eu/news/devops/145611/kiro-lets-ai-bots-continuously-build-software-autonomously/)

**Permissions and approvals**
- There are three permission modes. **Normal** asks for approval before actions, except for specified built-in operations. **Reads** allows reads without approval and asks before other changes. **Trust** acts without per-action approval, subject to security policies. Security policies and blocked commands apply in every mode — [How bots work](https://kiro.dev/docs/crew/bots/how-bots-work/)
- New bots start in Trust mode. A bot asks for your approval before changing its own model, permissions, goals, milestones, check-in schedules or worker abilities — [How bots work](https://kiro.dev/docs/crew/bots/how-bots-work/); [agentencode.de](https://agentencode.de/en/news/kiro-crew-bots-persistent-ai-agents-memory-approvals-2026/)

**Dashboards**
- Dashboards left Feature Preview, and the Dashboard tab is on for everyone. Every figure is read from the bot's crew log, and a value the bot wrote itself is tagged as such — [Crew changelog](https://kiro.dev/changelog/crew/); [Crew 0.10 changelog](https://kiro.dev/changelog/crew/0-10/)

**Interfaces / UX surfaces**
- You can use it through a desktop app, a web dashboard and the CLI, or through Slack, Discord, Telegram, Teams, Webex, WeCom and WeChat (Weixin) — [Crew docs](https://kiro.dev/docs/crew/)
- A "Gateway" routes work through those chat channels "without relocating the agent or its state" (third-party summary) — [chatgate.ai](https://chatgate.ai/post/kiro-crew); [datastudios](https://www.datastudios.org/post/aws-kiro-crew-bots-generally-available-memory-schedules)
- Desktop builds: macOS (signed .dmg), Windows (signed .exe installer), Linux (.deb, .rpm or .AppImage) — [Crew docs](https://kiro.dev/docs/crew/). An earlier tutorial said only a Mac desktop app existed at first — [kirotutorial](https://kirotutorial.com/kiro-crew/)
- Community usage: "Kiro Crew + Discord: Running My Agent From My Phone" — [DEV](https://dev.to/aws/kiro-crew-discord-running-my-agent-from-my-phone-4cb5)
- An AWS network-operations use case: "Building self-learning agents for intelligent Network Operations with Kiro Crew" — [AWS Industries blog](https://aws.amazon.com/blogs/industries/building-self-learning-agents-for-intelligent-network-operations-with-kiro-crew/)
- A LocalStack use case gives an AI agent "a Recurring AWS Testing Job" — [LocalStack blog](https://blog.localstack.cloud/what-is-kiro-crew/)

**Pricing**
- The Crew software is Apache-2.0 open source with no license fee. It runs on Kiro CLI, and model work consumes Kiro credits. A free Kiro account is required — [datastudios](https://www.datastudios.org/post/aws-kiro-crew-bots-generally-available-memory-schedules); [makerstack review](https://makerstack.co/reviews/kiro-crew-review/)
- From the bots docs, as quoted in search: "Agent runs consume credits, including worker activity and scheduled model calls". Scheduled jobs that run only plain scripts, such as health checks or simple git operations, consume zero credits — [How bots work](https://kiro.dev/docs/crew/bots/how-bots-work/); [datastudios](https://www.datastudios.org/post/aws-kiro-crew-bots-generally-available-memory-schedules)
- Bots draw on the same shared credit pool as interactive work — [beri.net: "Your Kiro Bots Spend the Same Credits as Interactive Work"](https://www.beri.net/article/aws-kiro-bots-ga-kiro-crew-scheduled-coding-agents-pooled-credits-permissions-memory-team-admin-limits)
- Plan figures from a third-party comparison: Free 50 credits; Pro $20 / 1,000; Pro+ $40 / 2,000; Pro Max $100 / 5,000; Power $200 / 10,000; add-on credits $0.04 each. Plan credits reset monthly, and add-on credits expire 12 months after purchase. Teams of two or more pool usage at $20 per seat per month, with 500 credits per seat — [datastudios](https://www.datastudios.org/post/aws-kiro-crew-bots-generally-available-memory-schedules); [beri.net](https://www.beri.net/article/aws-kiro-bots-ga-kiro-crew-scheduled-coding-agents-pooled-credits-permissions-memory-team-admin-limits). The Stork review gives different numbers ($20 Basic with 20 credits, $50 Pro, $0.05 per credit) — [Stork review](https://www.stork.ai/en/kiro-crew). Treat Stork as likely outdated.

**What users like**
- MakerStack scores Kiro Crew 8.0/10 ("Free to Start") — [makerstack review](https://makerstack.co/reviews/kiro-crew-review/)
- Third-party reviews frame its value as persistence: work "that keeps going when you step away", and one ongoing contact instead of one-off sessions — [Speaker Deck: Intro to Kiro Crew (Work that keeps going when you step away)](https://speakerdeck.com/inariku/kiro-crew-intro-en); [Speaker Deck: Bots in Kiro Crew](https://speakerdeck.com/inariku/kiro-crew-bots-en)
- A hands-on post uses bots to understand an unfamiliar codebase — [DEV: Understanding an Unfamiliar Codebase with Kiro Crew bots](https://dev.to/aws/understanding-an-unfamiliar-codebase-with-kiro-crew-bots-1gkg)
- On the related Kiro IDE, a Hacker News commenter wrote: "Kiro's spec based development is something that's new (for me atleast) and it's pretty good." — [Hacker News](https://news.ycombinator.com/threads?id=premn)

**What users dislike or criticize**
- Trust is the default permission mode, and Crew's default sandbox leaves `~/.ssh` and `~/.aws` visible to agent shell commands. This is a third-party critique — [DEV: "AWS's Kiro Bots Start in Trust Mode, and Crew's Default Sandbox Leaves ~/.ssh Visible"](https://dev.to/gabrielanhaia/awss-kiro-bots-start-in-trust-mode-and-crews-default-sandbox-leaves-ssh-visible-539f)
- One user on a pre-release build (0.12.0rc10) said the bot went idle after they approved milestones and needed a follow-up message to restart. They found that confusing for a product meant for unattended work — [DEV: Understanding an Unfamiliar Codebase with Kiro Crew bots](https://dev.to/aws/understanding-an-unfamiliar-codebase-with-kiro-crew-bots-1gkg)
- Cost: one review says "open source" here means free to read, not free to run, because every agent request draws on Kiro credits — [makerstack review](https://makerstack.co/reviews/kiro-crew-review/); [beri.net](https://www.beri.net/article/aws-kiro-bots-ga-kiro-crew-scheduled-coding-agents-pooled-credits-permissions-memory-team-admin-limits)
- Some UI strings still said "crewmate" after the rename to Bots — [Speaker Deck: Kiro updates October 2026](https://speakerdeck.com/inariku/kiro-update-2026-10-en)

### Inferences
- The "very useful" part most likely comes from the bot model. A bot is one named, long-lived contact that remembers, proposes a plan, runs on a schedule, delegates heavy work in the background, and reaches you on the chat app you already use, including on your phone through Discord or Slack.
- Kiro Crew is aimed at software development work, as its repo tagline "persistent workspace for development work" says. Its bot pattern of goals, milestones, check-ins, reports, memory and permission modes is domain-neutral, though, and maps closely onto a small-business AI office.

### Gaps
- Launch date: several third-party pages put the open-source launch at 2026-08-04 ([makerstack](https://makerstack.co/reviews/kiro-crew-review/)), but I could not confirm it against kiro.dev. Version history also conflicts across sources: one lists 0.1.2 as the open-source release, another describes 0.6.0 in September 2026, and the Mates → Bots rename is placed at 0.11.3 or 0.12.0.
- Pricing tiers come from third-party summaries. I could not read kiro.dev/pricing, so the figures should be checked there.
- I found no Reddit threads or App Store / Play Store reviews of Kiro Crew. User sentiment rests on a few DEV posts, a review site and slide decks.
- Some third-party claims arrived as one blended search summary, notably the Gateway wording and the plan figures. The source attribution for those is approximate.
- VentureBeat's full text was not available, so its exact wording on approvals is unverified.

## Q3. Ranking of candidates by likelihood

### Takeaway
1) **AWS Kiro Crew (Bots)**, very likely; 2) **AWS Kiro IDE/CLI**, possible if the user is mixing up the family; 3) **"Kiro: Sales AI ChatBot Agent" on Shopify**, possible only if "Crew" is misremembered; 4) **CrewAI**; 5) **Kroolo / Kroo AI**; 6) **Kiru by Stratamos**; 7) generic "bot" and "AI team" apps.

### Cited Findings
1. **Kiro Crew (AWS)**: an exact name match, and its user-facing agents are literally called "Bots" ([Crew changelog](https://kiro.dev/changelog/crew/)). Bots went GA on 2026-11-05, about two weeks before today ([VentureBeat](https://venturebeat.com/technology/aws-launches-agentic-ai-kiro-bots-with-memory-and-support-for-recurring-development-jobs)). It works through Slack, Discord and Telegram ([Crew docs](https://kiro.dev/docs/crew/)) and has wide coverage on DEV, AWS blogs and tech press ([Techzine](https://www.techzine.eu/news/devops/145611/kiro-lets-ai-bots-continuously-build-software-autonomously/); [ITdaily](https://itdaily.com/news/software/kiro-bots-manage-ai-agents/)).
2. **Kiro IDE/CLI (AWS)**: the same brand, and its spec, hooks and steering ideas are well known ([Introducing Kiro](https://kiro.dev/blog/introducing-kiro/)). It is an IDE, not a "crew" or a "bot app".
3. **Kiro: Sales AI ChatBot Agent (Shopify)**: it matches "Kiro" and "bot app" and is aimed at small merchants, but nothing about it is a "crew" ([Shopify App Store](https://apps.shopify.com/kiro)).
4. **CrewAI**: it matches "crew" and the multi-agent team concept ([GitHub crewAIInc/crewAI](https://github.com/crewAIInc/crewAI)), but it is a developer framework, not an app, and has no "Kiro" sound.
5. **Kroolo / Kroo AI**: a phonetic partial match ("Kroo" ≈ "crew") with an "AI Team" pitch ([Kroolo AI agent](https://corp.kroolo.com/ai/ai-agent)).
6. **Kiru by Stratamos**: a phonetic near-match to "Kiri" ([Product Hunt: Kiru](https://www.producthunt.com/p/kiru-by-stratamos/kiru)), but it is a single assistant, not a crew.
7. **Agentic Bots, Agent Teams AI, Teamily, TeamDay**: these match on concept only ([App Store: Agentic Bots](https://apps.apple.com/us/app/agentic-bots/id6753194867); [Agent Teams AI](https://raw.githubusercontent.com/777genius/agent-teams-ai/main/README.md)).

### Inferences
- The user builds a Rust/React product with AI coding agents, as the repo's AGENTS.md describes, so they are the developer audience Kiro Crew targets. That makes it even more likely they used Kiro Crew bots, perhaps while building Airobot itself.
- If the user remembers a customer-facing chatbot for a shop rather than a personal agent crew, the Shopify "Kiro" app would be the fallback. Confirming with the user takes one question: "the AWS one with bots in Slack/Discord, or a Shopify sales chatbot?"

### Gaps
- Popularity numbers such as GitHub stars, downloads or installs were not available for any candidate, so the ranking is qualitative.

## Q4. Ideas that transfer to a small-business AI office (Airobot)

### Takeaway
Kiro Crew's bot model is the richest source. Its main ideas are one persistent thread per bot, goal → milestone → check-in → report cards, background workers, editable memory and learned skills, permission modes plus approval for self-changes, recurring jobs set from chat, log-backed dashboards, and multi-channel access. Kiro IDE adds spec → tasks, steering files and event hooks. CrewAI, Kroolo and others add role templates and workflow ideas. Several of these map directly onto modules Airobot already has: group chat, bots, tasks, approvals, the budget gate, the procedure evolver and connectors.

### Cited Findings
**From Kiro Crew bots (strongest candidate)**
1. **One persistent thread per bot, with background workers.** The bot is a single point of contact. It handles short tasks itself and delegates long ones to workers that report back while you keep chatting — [How bots work](https://kiro.dev/docs/crew/bots/how-bots-work/); [Speaker Deck](https://speakerdeck.com/inariku/kiro-crew-bots-en)
2. **Goal → proposed measurable milestones → optional daily/weekly/monthly check-ins → saved report documents, each confirmed as a card** — [Bots docs](https://kiro.dev/docs/crew/bots/); [Crew 0.10 changelog](https://kiro.dev/changelog/crew/0-10/)
3. **Recurring jobs set in plain language from chat** (e.g. "draft release notes each Friday"). Script-only scheduled jobs cost zero credits, while model runs cost credits — [VentureBeat](https://venturebeat.com/technology/aws-launches-agentic-ai-kiro-bots-with-memory-and-support-for-recurring-development-jobs); [How bots work](https://kiro.dev/docs/crew/bots/how-bots-work/)
4. **Visible, editable memory and self-synthesized skills**, isolated per bot. Skills are distilled from repeated patterns and can be inspected, refined or removed — [Crew docs](https://kiro.dev/docs/crew/); [How bots work](https://kiro.dev/docs/crew/bots/how-bots-work/)
5. **Permission modes (Normal / Reads / Trust), plus mandatory approval before a bot changes its own model, permissions, goals, schedules or worker abilities.** Kiro's Trust-by-default drew public criticism — [How bots work](https://kiro.dev/docs/crew/bots/how-bots-work/); [DEV critique](https://dev.to/gabrielanhaia/awss-kiro-bots-start-in-trust-mode-and-crews-default-sandbox-leaves-ssh-visible-539f)
6. **Log-backed dashboards.** Every figure comes from the bot's log, and self-reported values are tagged — [Crew changelog](https://kiro.dev/changelog/crew/)
7. **Reach the same bot from many channels** (Slack, Discord, Telegram, Teams and others) without moving its state, and **choose a model per bot** to control cost — [Crew docs](https://kiro.dev/docs/crew/); [Introducing bots in Kiro Crew](https://kiro.dev/blog/crew-bots/)

**From Kiro IDE**
1. **Spec flow**: requirements (user stories with acceptance criteria) → design → a discrete task list, with task status tracked as the agent works — [Introducing Kiro](https://kiro.dev/blog/introducing-kiro/); [SD Times](https://sdtimes.com/ai/amazon-launches-spec-driven-ai-ide-kiro/). A source-code analysis suggests the agent asks the user whether the tasks look good before it proceeds — [Kiro agent source analysis](https://docsearch.algolia.com/mcp/docs/repo/ghuntley/amazon-kiro.kiro-agent-source-code-analysis) (implementation detail, not official).
2. **Steering files**: Markdown files in `.kiro/steering/` give the agent persistent project knowledge. Each file is either always included or included conditionally through front-matter — [DEV walkthrough: steering → specs → hooks](https://dev.to/alizgheib/kiro-from-steering-docs-to-specs-to-hooks-an-agentic-ide-walkthrough-3nbf)
3. **Agent hooks**: event-driven automations that run an agent task in the background on file save, create or delete, or on a manual trigger. They are configured as JSON in `.kiro/hooks/` — [DEV walkthrough](https://dev.to/alizgheib/kiro-from-steering-docs-to-specs-to-hooks-an-agentic-ide-walkthrough-3nbf)

**From CrewAI**
1. **Define agents by role, goal and an optional backstory.** The backstory gives the agent a point of view or tone — [GitHub crewAIInc/crewAI](https://github.com/crewAIInc/crewAI); [DigitalOcean CrewAI crash course](https://www.digitalocean.com/community/tutorials/crewai-crash-course-role-based-agent-orchestration)
2. **Separate Crews (open-ended teams) from Flows (event-driven workflows** with state, conditionals, loops and branching, which can call crews, single agents or plain code) — [GitHub crewAIInc/crewAI](https://github.com/crewAIInc/crewAI)
3. **The hosted platform meters live deployed crews and monthly executions**, a possible pricing model — [ZenML: CrewAI pricing](https://www.zenml.io/blog/crewai-pricing)

**From Kiro Shopify chatbot, Kroolo, Kiru**
1. **A customer-facing 24/7 store bot** that answers questions about products, orders, shipping and returns — [Shopify App Store: Kiro](https://apps.shopify.com/kiro)
2. **Pre-built role agents**, e.g. a Sales agent connected to Gmail that drafts cold outreach and follows up automatically, and a Marketing agent for blog posts, SEO descriptions and social captions. Kroolo lists 40+ agents and 30+ integrations — [Kroolo AI agent](https://corp.kroolo.com/ai/ai-agent); [Product Hunt: Kroolo](https://www.producthunt.com/products/kroolo/built-with)
3. **Ambient capture → system updates**: transcribe conversations and screenshots, update the CRM, trigger API actions. Priced at $14/seat/month plus pass-through model (Groq) API costs, with a 7-day trial — [Product Hunt: Kiru](https://www.producthunt.com/p/kiru-by-stratamos/kiru)

### Inferences
These map Airobot modules (from the repo's AGENTS.md) to borrowed ideas:
- **Bots + group chat.** Give each Airobot bot its own persistent thread next to the office group chat. Long jobs become server-run tasks through the existing task dispatcher that post results back to that thread. This mirrors Kiro's bot and worker split.
- **Approvals.** Use Airobot's existing approvals as Kiro-style confirm cards for goals, milestones, check-in schedules, and any change a bot proposes to its own permissions, model or budget. Avoid Kiro's Trust-by-default: default new SMB bots to a "Normal"/"ask first" mode, since small-business owners are less able to judge risky actions.
- **Tasks + Kanban apps.** Kiro IDE's requirements → design → tasks flow becomes a "brief → plan → Kanban cards" flow. The owner approves the plan before the bot starts, and card status updates as work proceeds.
- **Business profile as steering files.** An always-included "about the business" profile (hours, tone, prices, policies) plus conditionally included files per app or connector, all editable by the owner.
- **Connectors as hooks.** Events such as a Stripe payment, a Shopify order, a webhook or a Google event trigger a bot task in the background. This is Kiro's hook idea applied to the connectors Airobot already has.
- **Budget gate.** Copy the split between zero-cost scripted schedules and credit-consuming model runs, show per-bot spend, and allow a cheaper model per bot.
- **Procedure evolver.** Kiro's "synthesize reusable skills from repeated patterns, then inspect, refine, or remove them" is close to Airobot's procedure evolver. The lesson is to keep learned procedures and memory visible and editable in plain text, scoped per bot.
- **Weekly report artifact + log-backed dashboard.** A weekly owner report in which every number traces to the activity log and self-reported figures are labeled. This builds trust with non-technical owners.
- **Role templates** (Kroolo, CrewAI): ship pre-built Sales, Marketing and Support bots, each defined by role, goal and backstory, so owners start from a template rather than a blank prompt.

### Gaps
- I could not verify whether Kiro Crew bots can hand work to one another or collaborate in a shared group thread. The docs summary stresses isolation between bots. Whether a "crew" of bots coordinates in a group, as Airobot's Architect does, is unconfirmed.
- No usability data (retention, satisfaction scores) exists for any candidate, so "why users find it useful" rests on qualitative reviews.
- I did not verify CrewAI's current hosted price points or its process types; search returned no official figures.
