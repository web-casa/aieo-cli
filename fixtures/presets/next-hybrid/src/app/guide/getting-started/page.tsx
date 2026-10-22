import type { Metadata } from 'next'
import { LOREM, Nav } from '../../site'

export const metadata: Metadata = {
  title: 'Getting Started Guide | Example Inc',
  description: 'A short getting started guide for the Example Inc fixture site.',
  alternates: { canonical: '/guide/getting-started' },
}

export default function GettingStarted() {
  return (
    <main>
      <Nav />
      <h1>Getting started</h1>
      <p>{LOREM}</p>
    </main>
  )
}
