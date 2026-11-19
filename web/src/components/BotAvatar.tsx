import React from 'react';

// A bot's avatar: its first letter in white on a coloured circle, like the
// app icon. The colour comes from the bot's id, so it never changes and
// neighbouring bots usually differ.
const COLOURS = ['#2563eb', '#7c3aed', '#db2777', '#ea580c', '#16a34a', '#0891b2', '#ca8a04', '#475569'];

export const avatarColour = (key: string): string => {
  // FNV-1a with a final mix, so similar keys still spread over the colours.
  let h = 0x811c9dc5;
  for (let i = 0; i < key.length; i++) {
    h ^= key.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  h ^= h >>> 16;
  h = Math.imul(h, 0x45d9f3b) >>> 0;
  h = (h ^ (h >>> 16)) >>> 0;
  return COLOURS[h % COLOURS.length];
};

export const BotAvatar: React.FC<{ name: string; id?: string; size?: number }> = ({ name, id, size = 32 }) => (
  <span
    aria-hidden="true"
    className="rounded-full flex items-center justify-center flex-shrink-0 font-semibold text-white select-none"
    style={{
      width: size,
      height: size,
      fontSize: Math.round(size * 0.42),
      background: avatarColour(id || name),
      color: '#ffffff',
    }}
  >
    {name.replace(/^@/, '').charAt(0).toUpperCase() || '?'}
  </span>
);
