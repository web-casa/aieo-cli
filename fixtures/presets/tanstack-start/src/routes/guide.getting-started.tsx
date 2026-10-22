import { createFileRoute } from '@tanstack/react-router'
import { LOREM, Nav, seo } from '../site'

export const Route = createFileRoute('/guide/getting-started')({
  head: () =>
    seo('/guide/getting-started', 'Getting Started Guide | Example Inc', 'A short getting started guide for the Example Inc fixture site.'),
  component: GettingStarted,
})

function GettingStarted() {
  return (
    <main>
      <Nav />
      <h1>Getting started</h1>
      <p>{LOREM}</p>
    </main>
  )
}
