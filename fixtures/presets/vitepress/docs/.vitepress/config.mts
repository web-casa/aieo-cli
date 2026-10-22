import { defineConfig } from 'vitepress'

const ORIGIN = 'https://example.com'

export default defineConfig({
  lang: 'en',
  title: 'Example Inc',
  titleTemplate: false,
  description: 'Example Inc test site used for SEO checker fixtures.',
  cleanUrls: true,
  sitemap: { hostname: ORIGIN },
  themeConfig: {
    nav: [
      { text: 'Home', link: '/' },
      { text: 'About', link: '/about' },
      { text: 'Getting started', link: '/guide/getting-started' },
      { text: 'Hello world', link: '/blog/hello-world' },
    ],
  },
  transformPageData(pageData) {
    // index.md -> "/", foo/bar.md -> "/foo/bar" (cleanUrls form)
    const path = '/' + pageData.relativePath.replace(/(^|\/)index\.md$/, '$1').replace(/\.md$/, '')
    pageData.frontmatter.head ??= []
    pageData.frontmatter.head.push(['link', { rel: 'canonical', href: ORIGIN + path }])
  },
})
