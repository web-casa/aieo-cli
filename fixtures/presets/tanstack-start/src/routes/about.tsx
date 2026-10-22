import { createFileRoute } from '@tanstack/react-router'
import { LOREM, Nav, seo } from '../site'

export const Route = createFileRoute('/about')({
  head: () => seo('/about', 'About | Example Inc', 'Learn about Example Inc, the fictional company behind this fixture site.'),
  component: About,
})

function About() {
  return (
    <main>
      <Nav />
      <h1>About Example Inc</h1>
      <p>{LOREM}</p>
    </main>
  )
}
