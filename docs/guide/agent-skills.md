# Agent Skills

Codemark ships with a skill you can install into your coding agent. Once it's in, the agent knows how to create and recall structural bookmarks on its own, so the context it built up in one session is still there in the next, instead of being rebuilt from scratch every time.

## Installation

Install the skill globally for your preferred agent (e.g. Claude Code):
```bash
codemark install-skill --agent claude --scope user
```

## Example Prompts

Once installed, you can ask your agent to create or recall context in any session.

### Creating Context
> *"Trace how a request flows from the HTTP router to the database. Create a collection called `request-lifecycle` and bookmark each key hop: the route handler, the auth middleware, the service layer, and the query builder. Add a short note to each explaining its role."*

### Recalling Context
Later, in a fresh session (and even after the code has moved around), you or another agent can pick the context back up:

> *"Load the `request-lifecycle` collection and walk me through it."*

## Use Cases

Once a flow lives in a collection, anyone can reuse it: you, a teammate, or the next agent session:

- 🧭 **Onboard a new engineer**
  > *"Load the `request-lifecycle` collection and give me a guided tour of how this service handles a request, in the order the code runs."*
- 🔎 **Explain a code flow**
  > *"Bookmark the steps of the checkout flow into a `checkout` collection, then summarize what each step is responsible for."*
- 🐞 **Hunt a bug in a known flow**
  > *"There's a bug where expired tokens are still accepted. Read the `auth-flow` collection and tell me which hop is most likely responsible."*
- 🔗 **Relate two flows**
  > *"Compare the `request-lifecycle` and `background-jobs` collections. Where do they share code or state, and where could they conflict?"*
