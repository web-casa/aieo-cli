import type { NextConfig } from 'next'

const nextConfig: NextConfig = {
  output: 'export',
  trailingSlash: false,
  // Intentionally present: redirects are NOT supported with output: 'export'.
  async redirects() {
    return [{ source: '/old-page', destination: '/about', permanent: true }]
  },
}

export default nextConfig
