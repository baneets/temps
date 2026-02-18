import { defineConfig } from '@hey-api/openapi-ts'

export default defineConfig({
  input: 'openapi.json',
  output: {
    path: 'src/api/generated',
    format: 'prettier',
    lint: 'eslint',
  },
  client: '@hey-api/client-fetch',
  plugins: [
    '@hey-api/sdk',
    '@hey-api/typescript',
  ],
})
