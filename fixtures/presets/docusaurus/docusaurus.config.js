// @ts-check

/** @type {import('@docusaurus/types').Config} */
const config = {
  title: 'Example Inc',
  tagline: 'Example Inc test site used for SEO checker fixtures.',
  url: 'https://example.com',
  baseUrl: '/',
  trailingSlash: false,
  onBrokenLinks: 'throw',
  i18n: { defaultLocale: 'en', locales: ['en'] },
  presets: [
    [
      'classic',
      /** @type {import('@docusaurus/preset-classic').Options} */
      ({
        docs: {},
        blog: { showReadingTime: false, onUntruncatedBlogPosts: 'ignore' },
        theme: {},
        sitemap: { changefreq: null, priority: null },
      }),
    ],
  ],
  themeConfig: {
    navbar: {
      title: 'Example Inc',
      items: [
        { to: '/about', label: 'About', position: 'left' },
        { to: '/docs/guide/getting-started', label: 'Getting started', position: 'left' },
        { to: '/blog/hello-world', label: 'Hello world', position: 'left' },
      ],
    },
  },
}

module.exports = config
