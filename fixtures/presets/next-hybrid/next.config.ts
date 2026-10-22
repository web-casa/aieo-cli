import type { NextConfig } from 'next'

const nextConfig: NextConfig = {
  trailingSlash: false,
  async redirects() {
    return [{ source: '/old-page', destination: '/about', permanent: true }]
  },
}

export default nextConfig
