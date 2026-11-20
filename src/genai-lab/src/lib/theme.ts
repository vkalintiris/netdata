import { useSyncExternalStore } from 'react'
import { safeStorage } from './storage'

export type Theme = 'light' | 'dark'
const KEY = 'genai-lab-theme'
const listeners = new Set<() => void>()

function initial(): Theme {
  const saved = safeStorage.getItem(KEY)
  if (saved === 'light' || saved === 'dark') return saved
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}

let current: Theme = initial()
document.documentElement.classList.toggle('dark', current === 'dark')

function set(theme: Theme) {
  current = theme
  document.documentElement.classList.toggle('dark', theme === 'dark')
  safeStorage.setItem(KEY, theme)
  for (const l of listeners) l()
}

// One theme for the whole page; charts re-read their colors when it changes.
export function useTheme(): [Theme, () => void] {
  const theme = useSyncExternalStore(
    (l) => {
      listeners.add(l)
      return () => listeners.delete(l)
    },
    () => current,
  )
  return [theme, () => set(current === 'dark' ? 'light' : 'dark')]
}
