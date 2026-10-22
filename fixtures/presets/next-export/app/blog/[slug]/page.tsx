import type { Metadata } from 'next'
import { notFound } from 'next/navigation'
import { LOREM, Nav } from '../../site'

const posts: Record<string, { title: string; description: string }> = {
  'hello-world': {
    title: 'Hello World',
    description: 'The first blog post on the Example Inc fixture site, saying hello to the world.',
  },
}

export const dynamicParams = false

export function generateStaticParams() {
  return Object.keys(posts).map((slug) => ({ slug }))
}

export async function generateMetadata({ params }: { params: Promise<{ slug: string }> }): Promise<Metadata> {
  const { slug } = await params
  const post = posts[slug]
  return {
    title: `${post.title} | Example Inc Blog`,
    description: post.description,
    alternates: { canonical: `/blog/${slug}` },
  }
}

export default async function BlogPost({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params
  const post = posts[slug]
  if (!post) notFound()
  return (
    <main>
      <Nav />
      <h1>{post.title}</h1>
      <p>{LOREM}</p>
    </main>
  )
}
