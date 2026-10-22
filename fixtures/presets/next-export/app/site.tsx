import Link from 'next/link'

export const ORIGIN = 'https://example.com'

export const LOREM =
  'This example page exists so that an SEO checker can read real framework build output. It has a unique title, a meta description, a canonical link and a single top level heading. The navigation below links every other page of the site, which lets a crawler discover the full set of routes without needing a sitemap file at all.'

export function Nav() {
  return (
    <nav>
      <Link href="/">Home</Link> | <Link href="/about">About</Link> |{' '}
      <Link href="/guide/getting-started">Getting started</Link> |{' '}
      <Link href="/blog/hello-world">Hello world</Link>
    </nav>
  )
}
