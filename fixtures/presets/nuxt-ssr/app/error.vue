<script setup lang="ts">
import type { NuxtError } from '#app'
const props = defineProps<{ error: NuxtError }>()
useHead({
  title: 'Page Not Found | Example Inc',
  meta: [{ name: 'description', content: 'The requested page could not be found on the Example Inc fixture site.' }],
})
</script>

<template>
  <main>
    <SiteNav />
    <h1>Page not found</h1>
    <p>Sorry, the page you are looking for does not exist ({{ props.error.statusCode }}).</p>
  </main>
</template>
