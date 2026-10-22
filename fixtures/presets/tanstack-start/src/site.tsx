import { Link } from '@tanstack/react-router'

export const ORIGIN = 'https://example.com'
export const LOREM =
  'This example page exists so that an SEO checker can read real framework build output. It has a unique title, a meta description, a canonical link and a single top level heading. The navigation below links every other page of the site, which lets a crawler discover the full set of routes without needing a sitemap file at all.'

export function seo(path: string, title: string, description: string) {
  return {
    meta: [{ title }, { name: 'description', content: description }],
    links: [{ rel: 'canonical', href: ORIGIN + path }],
  }
}

export function Nav() {
  return (
    <nav>
      <Link to="/">Home</Link> | <Link to="/about">About</Link> |{' '}
      <Link to="/guide/getting-started">Getting started</Link> |{' '}
      <Link to="/blog/$slug" params={{ slug: 'hello-world' }}>
        Hello world
      </Link>
    </nav>
  )
}
