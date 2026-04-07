# Note & Context Templates

When adding a bookmark, follow these templates to ensure high-quality context that survives session transitions.

## 1. Note Template (Summary)
The `--note` should be concise and focus on the purpose of the code. **Do not repeat the node name or type**, as these are already captured by `codemark`.

**Format:** `<Action/Role>: <Brief Description>`
- **<Action/Role>**: The primary function or role of this code (e.g., Auth Validator, Configuration, Entry point).
- **<Brief Description>**: Why this piece of code is important or what it does.

*Example:* `Auth Validator: Handles all JWT verification for incoming requests.`

## 2. Context Template (Extended)
Include rationale and relationships within the note to provide deeper context.

**Format:** `Context: <Why this matters> | Relationships: <How it relates to other bookmarks>`

*Example:* `Core auth validator. Context: entry point for all signed requests. Relationships: depends on Claims struct.`

## 3. Tagging Strategy
Use structured, colon-prefixed tags for powerful filtering:
- `feature:<name>` — e.g., `feature:auth`, `feature:logging`
- `layer:<name>` — e.g., `layer:api`, `layer:db`, `layer:ui`
- `role:<name>` — e.g., `role:entrypoint`, `role:config`, `role:constant`
- `task:<id>` — e.g., `task:fix-123`, `task:refactor`

*Example:* `--tag feature:auth --tag role:entrypoint --tag layer:logic`
