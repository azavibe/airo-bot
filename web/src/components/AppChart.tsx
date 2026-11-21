import React, { useMemo, useState } from 'react';
import { OfficeRecord } from '../types';

// An app's `chart` view: one series read from the app's records. `x_axis`
// is a field (or `status`); dates group by month, other values by their
// text, in the order the schema's stages give when it names them.
// `y_axis` sums a number field, or counts records when absent or "count".
// `chart_type` is `bar` (default) or `spline` (a line over time). One
// accent colour, thin marks, a tooltip on hover or focus, and a table view.

export interface ChartView {
  id: string;
  title: string;
  record_type: string;
  chart_type?: 'bar' | 'spline';
  x_axis?: string;
  y_axis?: string;
  /** The order of the x values (an app's stages). */
  order?: string[];
}

const ISO_DATE = /^\d{4}-\d{2}-\d{2}/;

const monthLabel = (key: string) => {
  const [y, m] = key.split('-').map(Number);
  return new Date(y, m - 1, 1).toLocaleDateString(undefined, { month: 'short', year: '2-digit' });
};

const compact = (n: number) =>
  Math.abs(n) >= 1_000_000
    ? `${(n / 1_000_000).toFixed(1).replace(/\.0$/, '')}M`
    : Math.abs(n) >= 10_000
    ? `${(n / 1_000).toFixed(1).replace(/\.0$/, '')}K`
    : n.toLocaleString(undefined, { maximumFractionDigits: 1 });

export const chartPoints = (view: ChartView, records: OfficeRecord[]) => {
  const xKey = view.x_axis || 'status';
  const yKey = view.y_axis && view.y_axis !== 'count' ? view.y_axis : null;
  const rows = records.filter((r) => r.record_type === view.record_type);
  const valueOf = (r: OfficeRecord) =>
    xKey === 'status' || xKey === 'stage' ? r.data?.[xKey] ?? r.status : r.data?.[xKey];
  const raw = rows.map(valueOf).filter((v) => v !== undefined && v !== null && String(v).trim() !== '');
  const byDate = raw.length > 0 && raw.every((v) => ISO_DATE.test(String(v)));
  const sums = new Map<string, number>();
  for (const r of rows) {
    const v = valueOf(r);
    if (v === undefined || v === null || String(v).trim() === '') continue;
    const key = byDate ? String(v).slice(0, 7) : String(v);
    const add = yKey ? Number(r.data?.[yKey]) || 0 : 1;
    sums.set(key, (sums.get(key) ?? 0) + add);
  }
  let keys = [...sums.keys()];
  if (byDate) {
    keys.sort();
    // Fill the months between, so time reads evenly.
    if (keys.length > 1) {
      const filled: string[] = [];
      let [y, m] = keys[0].split('-').map(Number);
      const last = keys[keys.length - 1];
      for (let i = 0; i < 60; i++) {
        const k = `${y}-${String(m).padStart(2, '0')}`;
        filled.push(k);
        if (k >= last) break;
        m += 1;
        if (m > 12) {
          m = 1;
          y += 1;
        }
      }
      keys = filled;
    }
  } else if (view.order?.length) {
    const rank = (k: string) => {
      const i = view.order!.indexOf(k);
      return i === -1 ? view.order!.length : i;
    };
    keys.sort((a, b) => rank(a) - rank(b));
  } else {
    keys.sort((a, b) => (sums.get(b) ?? 0) - (sums.get(a) ?? 0));
  }
  return {
    byDate,
    measure: yKey ? yKey.replace(/_/g, ' ') : 'records',
    points: keys.map((k) => ({ key: k, label: byDate ? monthLabel(k) : k, value: sums.get(k) ?? 0 })),
  };
};

const W = 900;
const H = 260;
const PAD = { top: 16, right: 12, bottom: 34, left: 44 };

export const AppChart: React.FC<{ view: ChartView; records: OfficeRecord[] }> = ({ view, records }) => {
  const [hover, setHover] = useState<number | null>(null);
  const [asTable, setAsTable] = useState(false);
  const { points, measure, byDate } = useMemo(() => chartPoints(view, records), [view, records]);
  const kind = view.chart_type === 'spline' ? 'spline' : 'bar';

  if (points.length === 0) {
    return (
      <div className="glass-card p-8 text-center text-xs text-neutral-500">
        Nothing to chart yet. It fills in as records are added.
      </div>
    );
  }

  const max = Math.max(...points.map((p) => p.value), 0);
  // A rounded top for the axis, from 1, 2 or 5 times a power of ten.
  const step = (() => {
    const raw = (max || 1) / 4;
    const p = 10 ** Math.floor(Math.log10(raw));
    return [1, 2, 5, 10].map((f) => f * p).find((s) => s >= raw) ?? raw;
  })();
  const top = step * 4;
  const plotW = W - PAD.left - PAD.right;
  const plotH = H - PAD.top - PAD.bottom;
  const band = plotW / points.length;
  const barW = Math.max(4, Math.min(24, band - 2));
  const x = (i: number) => PAD.left + band * i + band / 2;
  const y = (v: number) => PAD.top + plotH - (v / top) * plotH;
  const labelEvery = Math.ceil(points.length / 8);
  const total = points.reduce((a, p) => a + p.value, 0);
  const shown = hover !== null ? points[hover] : null;

  return (
    <div className="glass-card p-4 space-y-2">
      <div className="flex items-baseline justify-between gap-3">
        <div className="min-w-0">
          <div className="text-sm font-semibold text-white truncate">{view.title}</div>
          <div className="text-[11px] text-neutral-400">
            {shown
              ? `${shown.label}: ${compact(shown.value)} ${measure}`
              : `${compact(total)} ${measure} in all`}
          </div>
        </div>
        <button
          type="button"
          onClick={() => setAsTable((t) => !t)}
          className="text-[11px] text-neutral-400 hover:text-white shrink-0"
        >
          {asTable ? 'Show chart' : 'Show as table'}
        </button>
      </div>

      {asTable ? (
        <table className="w-full text-[12px]">
          <thead>
            <tr className="text-neutral-400 text-left">
              <th className="font-medium py-1">{byDate ? 'Month' : (view.x_axis || 'status').replace(/_/g, ' ')}</th>
              <th className="font-medium py-1 text-right capitalize">{measure}</th>
            </tr>
          </thead>
          <tbody>
            {points.map((p) => (
              <tr key={p.key} className="border-t border-white/5 text-neutral-200">
                <td className="py-1">{p.label}</td>
                <td className="py-1 text-right tabular-nums">{p.value.toLocaleString()}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <svg
          viewBox={`0 0 ${W} ${H}`}
          className="w-full h-auto"
          role="img"
          aria-label={`${view.title}: ${points.map((p) => `${p.label} ${p.value}`).join(', ')}`}
          onMouseLeave={() => setHover(null)}
        >
          {/* Recessive grid and y ticks. */}
          {[0, 1, 2, 3, 4].map((i) => (
            <g key={i}>
              <line
                x1={PAD.left}
                x2={W - PAD.right}
                y1={y(step * i)}
                y2={y(step * i)}
                className={i === 0 ? 'stroke-white/25' : 'stroke-white/[0.07]'}
                strokeWidth={1}
              />
              <text x={PAD.left - 8} y={y(step * i) + 4} textAnchor="end" className="fill-neutral-500 text-[11px]">
                {compact(step * i)}
              </text>
            </g>
          ))}
          {points.map((p, i) =>
            i % labelEvery === 0 ? (
              <text key={p.key} x={x(i)} y={H - 12} textAnchor="middle" className="fill-neutral-500 text-[11px]">
                {p.label.length > 12 ? `${p.label.slice(0, 11)}…` : p.label}
              </text>
            ) : null
          )}

          {kind === 'bar' ? (
            points.map((p, i) => {
              const h = Math.max(0, y(0) - y(p.value));
              const r = Math.min(4, h, barW / 2);
              const left = x(i) - barW / 2;
              const topY = y(0) - h;
              // Rounded data end, square at the baseline.
              const d =
                h <= 0
                  ? ''
                  : `M${left},${y(0)} V${topY + r} Q${left},${topY} ${left + r},${topY} H${left + barW - r} Q${left + barW},${topY} ${left + barW},${topY + r} V${y(0)} Z`;
              return (
                <path
                  key={p.key}
                  d={d}
                  className={hover === null || hover === i ? 'fill-accent' : 'fill-accent/40'}
                />
              );
            })
          ) : (
            <>
              <path
                d={points.map((p, i) => `${i === 0 ? 'M' : 'L'}${x(i)},${y(p.value)}`).join(' ')}
                fill="none"
                className="stroke-accent"
                strokeWidth={2}
                strokeLinejoin="round"
                strokeLinecap="round"
              />
              {points.map((p, i) => (
                <circle
                  key={p.key}
                  cx={x(i)}
                  cy={y(p.value)}
                  r={hover === i ? 5 : 4}
                  className="fill-accent"
                  style={{ stroke: 'rgb(var(--c-surface))' }}
                  strokeWidth={2}
                />
              ))}
            </>
          )}
          {hover !== null && kind === 'spline' && (
            <line
              x1={x(hover)}
              x2={x(hover)}
              y1={PAD.top}
              y2={y(0)}
              className="stroke-white/20"
              strokeWidth={1}
            />
          )}
          {/* Hit targets: the whole column, bigger than the mark. */}
          {points.map((p, i) => (
            <rect
              key={p.key}
              x={PAD.left + band * i}
              y={PAD.top}
              width={band}
              height={plotH}
              fill="transparent"
              tabIndex={0}
              aria-label={`${p.label}: ${p.value.toLocaleString()} ${measure}`}
              onMouseEnter={() => setHover(i)}
              onFocus={() => setHover(i)}
              onBlur={() => setHover(null)}
            />
          ))}
        </svg>
      )}
    </div>
  );
};
