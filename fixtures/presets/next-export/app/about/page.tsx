import type { Metadata } from 'next'
import { LOREM, Nav } from '../site'

export const metadata: Metadata = {
  title: 'About | Example Inc',
  description: 'Learn about Example Inc, the fictional company behind this fixture site.',
  alternates: { canonical: '/about' },
}

export default function About() {
  return (
    <main>
      <Nav />
      <h1>About Example Inc</h1>
      <p>{LOREM}</p>
    </main>
  )
}
