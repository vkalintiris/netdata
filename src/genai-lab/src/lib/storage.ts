import { useEffect, useState } from 'react'

/** localStorage that never throws (private mode, blocked storage). */
export const safeStorage = {
  getItem(key: string): string | null {
    try {
      return localStorage.getItem(key)
    } catch {
      return null
    }
  },
  setItem(key: string, value: string): void {
    try {
      localStorage.setItem(key, value)
    } catch {
      // Non-fatal: the preference just won't persist.
    }
  },
}

/** A string-valued preference persisted per browser. */
export function usePreference<T extends string>(key: string, fallback: T, allowed: readonly T[]): [T, (v: T) => void] {
  const [value, setValue] = useState<T>(() => {
    const saved = safeStorage.getItem(key)
    return saved && (allowed as readonly string[]).includes(saved) ? (saved as T) : fallback
  })
  useEffect(() => safeStorage.setItem(key, value), [key, value])
  return [value, setValue]
}
