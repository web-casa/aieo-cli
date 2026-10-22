import React from 'react'
import Head from '@docusaurus/Head'
import Link from '@docusaurus/Link'
import Layout from '@theme/Layout'

const org = {
  '@context': 'https://schema.org',
  '@type': 'Organization',
  name: 'Example Inc',
  url: 'https://example.com',
  logo: 'https://example.com/logo.png',
}

export default function Home(): React.JSX.Element {
  return (
    <Layout title="Home" description="Home page of the Example Inc test site used for SEO checker fixtures.">
      <Head>
        <script type="application/ld+json">{JSON.stringify(org)}</script>
      </Head>
      <main className="container margin-vert--lg">
        <h1>Welcome to Example Inc</h1>
        <p>This example page exists so that an SEO checker can read real framework build output. It has a unique title, a meta description, a canonical link and a single top level heading. The navigation below links every other page of the site, which lets a crawler discover the full set of routes without needing a sitemap file at all.</p>
        <nav>
          <Link to="/">Home</Link> | <Link to="/about">About</Link> |{' '}
          <Link to="/docs/guide/getting-started">Getting started</Link> |{' '}
          <Link to="/blog/hello-world">Hello world</Link>
        </nav>
      </main>
    </Layout>
  )
}
