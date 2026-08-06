import { defineConfig } from 'vitepress'
import { withMermaid } from 'vitepress-plugin-mermaid'

export default withMermaid(defineConfig({
  base: "/codemark/docs/",
  title: "Codemark",
  description: "Durable, semantic bookmarks for AI agents and humans",
  lastUpdated: true,
  cleanUrls: true,
  
  themeConfig: {
    search: {
      provider: 'local'
    },
    nav: [
      { text: 'Home', link: '/' },
      { text: 'Guide', link: '/guide/introduction' }
    ],

    sidebar: [
      {
        text: 'Overview',
        items: [
          { text: 'What is Codemark?', link: '/guide/introduction' },
          { text: 'Core Concepts', link: '/guide/core-concepts' },
        ]
      },
      {
        text: 'Quickstart',
        items: [
          { text: 'Installation', link: '/guide/getting-started' },
          { text: 'Agent Skill', link: '/guide/agent-skills' },
        ]
      },
      {
        text: 'Workflows',
        items: [
          { text: 'Workflows', link: '/guide/workflows' },
          { text: 'Agent Walkthrough', link: '/guide/agent-workflow-walkthrough' },
        ]
      },
      {
        text: 'How It Works',
        items: [
          { text: 'Tree-sitter', link: '/guide/tree-sitter' },
          { text: 'Query Generation', link: '/guide/query-generation' },
          { text: 'Health Status', link: '/guide/health-status' },
          { text: 'Repository Registry', link: '/guide/repository-registry' },
          { text: 'Semantic Search', link: '/guide/embeddings' },
        ]
      },
      {
        text: 'Reference',
        items: [
          { text: 'Keybindings & UI', link: '/guide/keybindings' },
          { text: 'Configuration', link: '/guide/configuration' },
          { text: 'Dynamic Grammars', link: '/guide/dynamic-grammars' },
          { text: 'CLI Reference', link: '/guide/cli-reference' },
          { text: 'Markdown Templates', link: '/guide/templates' },
        ]
      }
    ],

    socialLinks: [
      { icon: 'github', link: 'https://github.com/DanielCardonaRojas/codemark' }
    ],
    
    editLink: {
      pattern: 'https://github.com/DanielCardonaRojas/codemark/edit/main/docs/:path',
      text: 'Edit this page on GitHub'
    }
  }
}))

