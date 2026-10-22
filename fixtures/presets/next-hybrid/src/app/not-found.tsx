import type { Metadata } from 'next'
import { Nav } from './site'

export const metadata: Metadata = {
  title: 'Page Not Found | Example Inc',
  description: 'The requested page could not be found on the Example Inc fixture site.',
}

export default function NotFound() {
  return (
    <main>
      <Nav />
      <h1>Page not found</h1>
      <p>Sorry, the page you are looking for does not exist.</p>
    </main>
  )
}
