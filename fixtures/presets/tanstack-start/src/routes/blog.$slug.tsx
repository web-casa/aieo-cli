import { createFileRoute, notFound } from '@tanstack/react-router'
import { LOREM, Nav, seo } from '../site'

const posts: Record<string, { title: string; description: string }> = {
  'hello-world': {
    title: 'Hello World',
    description: 'The first blog post on the Example Inc fixture site, saying hello to the world.',
  },
}

export const Route = createFileRoute('/blog/$slug')({
  loader: ({ params }) => {
    const post = posts[params.slug]
    if (!post) throw notFound()
    return post
  },
  head: ({ params, loaderData }) =>
    loaderData ? seo(`/blog/${params.slug}`, `${loaderData.title} | Example Inc Blog`, loaderData.description) : {},
  component: BlogPost,
})

function BlogPost() {
  const post = Route.useLoaderData()
  return (
    <main>
      <Nav />
      <h1>{post.title}</h1>
      <p>{LOREM}</p>
    </main>
  )
}
