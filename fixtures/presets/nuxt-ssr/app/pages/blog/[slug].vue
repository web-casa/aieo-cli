<script setup lang="ts">
const posts: Record<string, { title: string; description: string }> = {
  'hello-world': {
    title: 'Hello World',
    description: 'The first blog post on the Example Inc fixture site, saying hello to the world.',
  },
}
const route = useRoute()
const slug = String(route.params.slug)
const post = posts[slug]
if (!post) {
  throw createError({ statusCode: 404, statusMessage: 'Post not found', fatal: true })
}
seoHead(`/blog/${slug}`, `${post.title} | Example Inc Blog`, post.description)
</script>

<template>
  <main>
    <SiteNav />
    <h1>{{ post!.title }}</h1>
    <p>{{ LOREM }}</p>
  </main>
</template>
