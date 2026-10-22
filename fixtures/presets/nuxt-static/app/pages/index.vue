<script setup lang="ts">
seoHead('/', 'Home | Example Inc', 'Home page of the Example Inc test site used for SEO checker fixtures.')
useHead({
  script: [
    {
      type: 'application/ld+json',
      innerHTML: JSON.stringify({
        '@context': 'https://schema.org',
        '@type': 'Organization',
        name: 'Example Inc',
        url: ORIGIN,
        logo: `${ORIGIN}/logo.png`,
      }),
    },
  ],
})
</script>

<template>
  <main>
    <SiteNav />
    <h1>Welcome to Example Inc</h1>
    <p>{{ LOREM }}</p>
  </main>
</template>
