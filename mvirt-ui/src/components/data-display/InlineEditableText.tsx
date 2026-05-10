import { useEffect, useRef, useState } from 'react'
import { cn } from '@/lib/utils'

interface InlineEditableTextProps {
  /** Current value (may be empty). */
  value: string | undefined | null
  /** What to render when the value is empty *and* the field isn't being edited. */
  placeholder?: string
  /** Optional className applied to the outer span / form. */
  className?: string
  /** Saved when the user blurs or hits Enter. Called with the trimmed value
   *  (empty string represents "clear"). Should return a Promise so the
   *  component can show a saving spinner / re-throw on error. */
  onSave: (value: string) => Promise<unknown>
  /** Optional ARIA label for the input (e.g. "Edit description for rule X"). */
  ariaLabel?: string
}

/**
 * Click-to-edit text. Reads as muted-foreground placeholder when empty;
 * clicking anywhere on the text turns it into an `<input>` that submits on
 * Enter / blur and bails on Escape. Used for rule and SG descriptions.
 */
export function InlineEditableText({
  value,
  placeholder = '–',
  className,
  onSave,
  ariaLabel,
}: InlineEditableTextProps) {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(value ?? '')
  const [saving, setSaving] = useState(false)
  const inputRef = useRef<HTMLInputElement | null>(null)

  // When the upstream value changes (e.g. another tab updated it), keep
  // the draft synced as long as we're not actively editing.
  useEffect(() => {
    if (!editing) setDraft(value ?? '')
  }, [value, editing])

  // Auto-focus when entering edit mode.
  useEffect(() => {
    if (editing) {
      inputRef.current?.focus()
      inputRef.current?.select()
    }
  }, [editing])

  const startEdit = (e: React.MouseEvent) => {
    e.stopPropagation()
    setDraft(value ?? '')
    setEditing(true)
  }

  const commit = async () => {
    const trimmed = draft.trim()
    const current = (value ?? '').trim()
    if (trimmed === current) {
      setEditing(false)
      return
    }
    setSaving(true)
    try {
      await onSave(trimmed)
      setEditing(false)
    } catch {
      // Caller's mutation surfaces the error elsewhere; keep the editor
      // open so the user can adjust and retry.
    } finally {
      setSaving(false)
    }
  }

  const cancel = () => {
    setDraft(value ?? '')
    setEditing(false)
  }

  if (editing) {
    return (
      <input
        ref={inputRef}
        aria-label={ariaLabel}
        type="text"
        value={draft}
        disabled={saving}
        onChange={(e) => setDraft(e.target.value)}
        onClick={(e) => e.stopPropagation()}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault()
            commit()
          } else if (e.key === 'Escape') {
            e.preventDefault()
            cancel()
          }
        }}
        className={cn(
          'w-full bg-secondary border border-border rounded px-2 py-1 text-sm focus:outline-none focus:ring-1 focus:ring-purple',
          saving && 'opacity-60',
          className,
        )}
      />
    )
  }

  const isEmpty = !value || value.trim() === ''
  return (
    <button
      type="button"
      onClick={startEdit}
      className={cn(
        'text-left text-sm w-full px-2 py-1 -mx-2 rounded hover:bg-secondary/50 transition-colors',
        isEmpty && 'text-muted-foreground italic',
        className,
      )}
      aria-label={ariaLabel ?? 'Edit'}
    >
      {isEmpty ? placeholder : value}
    </button>
  )
}
