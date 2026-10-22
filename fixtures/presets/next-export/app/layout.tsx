import type { Metadata } from 'next'
import { ORIGIN } from './site'

export const metadata: Metadata = {
  metadataBase: new URL(ORIGIN),
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  )
}
