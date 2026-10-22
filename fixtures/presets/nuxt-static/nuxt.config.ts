export default defineNuxtConfig({
  compatibilityDate: '2026-10-01',
  devtools: { enabled: false },
  telemetry: false,
  app: {
    head: {
      htmlAttrs: { lang: 'en' },
    },
  },
  routeRules: {
    '/old-page': { redirect: { to: '/about', statusCode: 301 } },
  },
})
