<script setup lang="ts">
import { computed, watchEffect } from 'vue'
import { useRoute } from 'vue-router'
import { LOREM } from '../site'

const posts: Record<string, { title: string; description: string }> = {
  'hello-world': {
    title: 'Hello World',
    description: 'The first blog post on the Example Inc fixture site, saying hello to the world.',
  },
}
const route = useRoute()
const post = computed(() => posts[String(route.params.slug)])
watchEffect(() => {
  if (post.value) {
    document.title = `${post.value.title} | Example Inc Blog`
    document.head.querySelector('meta[name="description"]')?.setAttribute('content', post.value.description)
  }
})
</script>

<template>
  <main v-if="post">
    <h1>{{ post.title }}</h1>
    <p>{{ LOREM }}</p>
  </main>
  <main v-else>
    <h1>Post not found</h1>
  </main>
</template>
