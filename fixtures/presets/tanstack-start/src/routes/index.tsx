import { createFileRoute } from '@tanstack/react-router'
import { LOREM, Nav, ORIGIN, seo } from '../site'

export const Route = createFileRoute('/')({
  head: () => ({
    ...seo('/', 'Home | Example Inc', 'Home page of the Example Inc test site used for SEO checker fixtures.'),
    scripts: [
      {
        type: 'application/ld+json',
        children: JSON.stringify({
          '@context': 'https://schema.org',
          '@type': 'Organization',
          name: 'Example Inc',
          url: ORIGIN,
          logo: `${ORIGIN}/logo.png`,
        }),
      },
    ],
  }),
  component: Home,
})

function Home() {
  return (
    <main>
      <Nav />
      <h1>Welcome to Example Inc</h1>
      <p>{LOREM}</p>
    </main>
  )
}
