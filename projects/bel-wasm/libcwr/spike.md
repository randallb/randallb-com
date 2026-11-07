# Coworker workspace spike

Status: exploratory work retained as a draft, not selected for merging.
Owner: Randall Bennett. Direction recorded November 6, 2026.

This branch preserves experiments with the workspace REPL, persistent runtime
identity, source-backed coworkers, check/reload, structured resource programs,
Looper, and the WLD&D participant descriptions. The examples also retain their
integer integration so the experiment remains reproducible.

Randall selected signed integer support for extraction onto a separate branch
from main. Its branch is `feat/rbel-integers`; the rest of this stack remains a
spike. Keeping this snapshot does not endorse its architecture or authorize
merging it. Earlier runtime/Looper review was paused; integer verification does
not establish readiness of the surrounding runtime work.

The WLD&D example describes a human player, a model dungeon master, and a software
dice helper. Its sample scoring calculates a supplied face plus a bonus. Model
calls, random dice, live human input, and saved game state are not implemented.

Source direction: [Randall's instruction to extract integers and retain the rest
as drafts](cwp://local.randalls-macbook-pro/runtimes/e8609410f60cdd3cd357fad57f9dda5c027e77ca5410198e36811fbad7939e04/messages/ee9e35a2a0ef47029b45efe4bea645e5).
