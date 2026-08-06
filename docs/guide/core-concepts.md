# Core Concepts

Codemark revolves around a few key primitives designed to make code navigation and context sharing durable.

## Bookmarks
A **Bookmark** in Codemark is not just a line number. It is a semantic anchor attached to a specific structural node in your code (like a function definition, a class, or a loop) using the [Tree-sitter](https://tree-sitter.github.io/tree-sitter/) Abstract Syntax Tree (AST).

Because it tracks the structure, if you add lines above your bookmark or refactor the code around it, Codemark knows how to locate the bookmark again. This is called **Smart Resolution**.

## Collections
A **Collection** is a grouping of bookmarks. You can think of it as a playlist or a guided tour of a specific flow in your codebase.

For example, you might create a `checkout-flow` collection that contains bookmarks for:
1. The API route handler
2. The authentication middleware
3. The database query
4. The payment gateway integration

## Worktrees & Repositories
Codemark is **Git-aware**. It automatically detects your current repository and tracks bookmarks across commits and branches. It understands Git worktrees, ensuring that your context isn't lost when you switch branches or check out a different worktree of the same repository.
