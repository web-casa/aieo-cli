import type { Metadata } from 'next'
import { LOREM, Nav } from '../site'

export const dynamic = 'force-dynamic'

export const metadata: Metadata = {
  title: 'Search | Example Inc',
  description: 'Search the Example Inc fixture site. This page is rendered on the server per request.',
  alternates: { canonical: '/search' },
}

export default async function Search({ searchParams }: { searchParams: Promise<{ q?: string }> }) {
  const { q } = await searchParams
  return (
    <main>
      <Nav />
      <h1>Search</h1>
      <p>{q ? `Results for "${q}".` : LOREM}</p>
    </main>
  )
}
