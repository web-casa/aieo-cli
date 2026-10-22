import type { Metadata } from 'next'
import { LOREM, Nav, ORIGIN } from './site'

export const metadata: Metadata = {
  title: 'Home | Example Inc',
  description: 'Home page of the Example Inc test site used for SEO checker fixtures.',
  alternates: { canonical: '/' },
}

const org = {
  '@context': 'https://schema.org',
  '@type': 'Organization',
  name: 'Example Inc',
  url: ORIGIN,
  logo: `${ORIGIN}/logo.png`,
}

export default function Home() {
  return (
    <main>
      <script type="application/ld+json" dangerouslySetInnerHTML={{ __html: JSON.stringify(org) }} />
      <Nav />
      <h1>Welcome to Example Inc</h1>
      <p>{LOREM}</p>
    </main>
  )
}
