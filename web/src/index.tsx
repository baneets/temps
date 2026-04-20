import ReactDOM from 'react-dom/client'
import App from './App'

const UI_PICKER_SRC = 'https://ui.sh/ui-picker.js'
if (!document.querySelector(`script[src="${UI_PICKER_SRC}"]`)) {
  const script = document.createElement('script')
  script.src = UI_PICKER_SRC
  script.async = true
  document.body.appendChild(script)
}

const rootEl = document.getElementById('root')
if (rootEl) {
  const root = ReactDOM.createRoot(rootEl)
  root.render(<App />)
}
