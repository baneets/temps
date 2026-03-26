import Nav from './components/nav'

export const metadata = {
  title: 'Hello World',
  description: 'Next.js Hello World',
}

export default function RootLayout({
  children,
}: {
  children: React.ReactNode
}) {
  return (
    <html lang="en">
      <body>
        <Nav />
        {children}
      </body>
    </html>
  )
}
