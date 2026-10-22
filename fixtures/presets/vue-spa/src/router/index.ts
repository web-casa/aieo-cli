import { createRouter, createWebHistory } from 'vue-router'
import { ORIGIN } from '../site'

declare module 'vue-router' {
  interface RouteMeta {
    title?: string
    description?: string
  }
}

const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      path: '/',
      name: 'home',
      component: () => import('../views/HomeView.vue'),
      meta: { title: 'Home | Example Inc', description: 'Home page of the Example Inc test site used for SEO checker fixtures.' },
    },
    {
      path: '/about',
      name: 'about',
      component: () => import('../views/AboutView.vue'),
      meta: { title: 'About | Example Inc', description: 'Learn about Example Inc, the fictional company behind this fixture site.' },
    },
    {
      path: '/guide/getting-started',
      name: 'getting-started',
      component: () => import('../views/GettingStartedView.vue'),
      meta: { title: 'Getting Started Guide | Example Inc', description: 'A short getting started guide for the Example Inc fixture site.' },
    },
    {
      path: '/blog/:slug',
      name: 'blog-post',
      component: () => import('../views/BlogPostView.vue'),
    },
    {
      path: '/:pathMatch(.*)*',
      name: 'not-found',
      component: () => import('../views/NotFoundView.vue'),
      meta: { title: 'Page Not Found | Example Inc', description: 'The requested page could not be found on the Example Inc fixture site.' },
    },
  ],
})

function setMeta(selector: string, attr: string, value: string) {
  const el = document.head.querySelector(selector)
  if (el) el.setAttribute(attr, value)
}

router.afterEach((to) => {
  if (to.meta.title) document.title = to.meta.title
  if (to.meta.description) setMeta('meta[name="description"]', 'content', to.meta.description)
  setMeta('link[rel="canonical"]', 'href', ORIGIN + to.path)
})

export default router
