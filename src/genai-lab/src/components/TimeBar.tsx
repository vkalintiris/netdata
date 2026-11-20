import { ChevronLeft, ChevronRight, ZoomOut } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { absolute, PRESETS, usePageState } from '@/lib/page-state'

const timeFmt = new Intl.DateTimeFormat(undefined, {
  month: 'short',
  day: 'numeric',
  hour: '2-digit',
  minute: '2-digit',
  second: '2-digit',
})

export function TimeBar() {
  const { window, setWindow } = usePageState()
  const isRelative = window.after < 0
  const preset = isRelative && window.before === 0 ? PRESETS.find((p) => p.after === window.after) : undefined

  const pan = (direction: number) => {
    const w = absolute(window)
    const step = ((w.before - w.after) / 2) * direction
    const now = Math.floor(Date.now() / 1000)
    if (w.before + step >= now) setWindow({ after: -(w.before - w.after), before: 0 })
    else setWindow({ after: w.after + step, before: w.before + step })
  }

  const zoomOut = () => {
    const w = absolute(window)
    const span = w.before - w.after
    const now = Math.floor(Date.now() / 1000)
    const before = Math.min(now, w.before + span / 2)
    if (before >= now) setWindow({ after: -span * 2, before: 0 })
    else setWindow({ after: before - span * 2, before })
  }

  const range = absolute(window)

  return (
    <div className="flex items-center gap-2">
      {!preset && (
        <span className="hidden font-mono text-xs text-muted-foreground lg:inline">
          {timeFmt.format(range.after * 1000)} – {isRelative ? 'now' : timeFmt.format(range.before * 1000)}
        </span>
      )}
      <Tooltip>
        <TooltipTrigger asChild>
          <Button variant="ghost" size="icon-sm" onClick={() => pan(-1)} aria-label="Earlier">
            <ChevronLeft />
          </Button>
        </TooltipTrigger>
        <TooltipContent>Earlier (or drag a chart)</TooltipContent>
      </Tooltip>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button variant="ghost" size="icon-sm" onClick={() => pan(1)} aria-label="Later" disabled={isRelative && window.before === 0}>
            <ChevronRight />
          </Button>
        </TooltipTrigger>
        <TooltipContent>Later</TooltipContent>
      </Tooltip>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button variant="ghost" size="icon-sm" onClick={zoomOut} aria-label="Zoom out">
            <ZoomOut />
          </Button>
        </TooltipTrigger>
        <TooltipContent>Zoom out (Shift + drag on a chart to zoom in)</TooltipContent>
      </Tooltip>
      <ToggleGroup
        type="single"
        variant="outline"
        size="sm"
        value={preset?.label ?? ''}
        onValueChange={(label) => {
          const p = PRESETS.find((x) => x.label === label)
          if (p) setWindow({ after: p.after, before: 0 })
        }}
      >
        {PRESETS.map((p) => (
          <ToggleGroupItem key={p.label} value={p.label} className="px-2.5 font-mono text-xs">
            {p.label}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
    </div>
  )
}
