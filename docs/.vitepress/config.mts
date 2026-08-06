import { defineConfig } from 'vitepress'

export default defineConfig({
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
        text: 'Getting Started',
        items: [
          { text: 'What is Codemark?', link: '/guide/introduction' },
          { text: 'Installation & Quickstart', link: '/guide/getting-started' },
          { text: 'Core Concepts', link: '/guide/core-concepts' },
        ]
      },
      {
        text: 'Usage & Workflows',
        items: [
          { text: 'Workflows', link: '/guide/workflows' },
          { text: 'Agent Skills', link: '/guide/agent-skills' },
        ]
      },
      {
        text: 'Reference',
        items: [
          { text: 'Keybindings & UI', link: '/guide/keybindings' },
          { text: 'Configuration', link: '/guide/configuration' },
        ]
      },
      {
        text: 'Architecture',
        items: [
          { text: 'Under the Hood', link: '/guide/under-the-hood' },
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
})
