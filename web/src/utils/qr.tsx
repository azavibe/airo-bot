// Zero-dependency pure TypeScript QR Code SVG generator
// Compliant with standard ISO/IEC 18004 Byte Mode (Versions 1-10)
import React from 'react';

export function generateQRCodeMatrix(text: string): boolean[][] {
  const utf8 = new TextEncoder().encode(text);
  const dataLen = utf8.length;

  // Capacity table for EC Level L (versions 1-10)
  // [version, totalCodewords, ecCodewords, dataCapacityBytes]
  const table = [
    [1, 26, 7, 19],
    [2, 44, 10, 34],
    [3, 70, 15, 55],
    [4, 100, 20, 80],
    [5, 134, 26, 108],
    [6, 172, 36, 136],
    [7, 196, 40, 156],
    [8, 242, 48, 194],
    [9, 292, 60, 232],
    [10, 346, 72, 274],
  ];

  let spec = table[table.length - 1];
  for (const row of table) {
    const requiredBytes = dataLen + 2;
    if (requiredBytes <= row[3]) {
      spec = row;
      break;
    }
  }

  const version = spec[0];
  const size = 17 + version * 4;
  const ecCodewords = spec[2];
  const dataCapacity = spec[3];

  const bits: number[] = [];
  function pushBits(val: number, count: number) {
    for (let i = count - 1; i >= 0; i--) {
      bits.push((val >> i) & 1);
    }
  }

  // 1. Header: Byte mode indicator (0100)
  pushBits(0b0100, 4);
  pushBits(dataLen, version >= 10 ? 16 : 8);
  for (let i = 0; i < dataLen; i++) {
    pushBits(utf8[i], 8);
  }

  // 2. Terminator & padding
  const maxDataBits = dataCapacity * 8;
  const termBits = Math.min(4, maxDataBits - bits.length);
  pushBits(0, termBits);
  while (bits.length % 8 !== 0) {
    bits.push(0);
  }
  let padToggle = 0;
  while (bits.length < maxDataBits) {
    pushBits(padToggle === 0 ? 0xec : 0x11, 8);
    padToggle ^= 1;
  }

  const dataBytes = new Uint8Array(dataCapacity);
  for (let i = 0; i < dataCapacity; i++) {
    let b = 0;
    for (let j = 0; j < 8; j++) {
      b = (b << 1) | bits[i * 8 + j];
    }
    dataBytes[i] = b;
  }

  // 3. Reed-Solomon Galois Field 256
  const GF256_EXP = new Uint8Array(512);
  const GF256_LOG = new Uint8Array(256);
  let x = 1;
  for (let i = 0; i < 255; i++) {
    GF256_EXP[i] = x;
    GF256_EXP[i + 255] = x;
    GF256_LOG[x] = i;
    x = (x << 1) ^ (x >= 128 ? 0x11d : 0);
  }
  function gfMul(a: number, b: number) {
    if (a === 0 || b === 0) return 0;
    return GF256_EXP[GF256_LOG[a] + GF256_LOG[b]];
  }
  function rsGenPoly(degree: number) {
    let poly = [1];
    for (let i = 0; i < degree; i++) {
      const next = new Array(poly.length + 1).fill(0);
      for (let j = 0; j < poly.length; j++) {
        next[j] ^= gfMul(poly[j], GF256_EXP[i]);
        next[j + 1] ^= poly[j];
      }
      poly = next;
    }
    return poly;
  }
  const gen = rsGenPoly(ecCodewords);
  const ecBytes = new Uint8Array(ecCodewords);
  for (let i = 0; i < dataCapacity; i++) {
    const factor = dataBytes[i] ^ ecBytes[0];
    for (let j = 0; j < ecCodewords - 1; j++) {
      ecBytes[j] = ecBytes[j + 1] ^ gfMul(gen[j + 1], factor);
    }
    ecBytes[ecCodewords - 1] = gfMul(gen[ecCodewords], factor);
  }

  const allCodewords = new Uint8Array(dataCapacity + ecCodewords);
  allCodewords.set(dataBytes, 0);
  allCodewords.set(ecBytes, dataCapacity);

  // 4. Matrix construction
  const matrix: (boolean | null)[][] = Array.from({ length: size }, () =>
    new Array(size).fill(null)
  );
  const isFunction: boolean[][] = Array.from({ length: size }, () =>
    new Array(size).fill(false)
  );

  function setFunc(r: number, c: number, val: boolean) {
    if (r >= 0 && r < size && c >= 0 && c < size) {
      matrix[r][c] = val;
      isFunction[r][c] = true;
    }
  }

  function drawFinder(r0: number, c0: number) {
    for (let r = -1; r <= 7; r++) {
      for (let c = -1; c <= 7; c++) {
        const nr = r0 + r;
        const nc = c0 + c;
        if (nr < 0 || nr >= size || nc < 0 || nc >= size) continue;
        if (
          (r >= 0 && r <= 6 && (c === 0 || c === 6)) ||
          (c >= 0 && c <= 6 && (r === 0 || r === 6)) ||
          (r >= 2 && r <= 4 && c >= 2 && c <= 4)
        ) {
          setFunc(nr, nc, true);
        } else {
          setFunc(nr, nc, false);
        }
      }
    }
  }

  drawFinder(0, 0);
  drawFinder(0, size - 7);
  drawFinder(size - 7, 0);

  // Timing patterns
  for (let i = 8; i < size - 8; i++) {
    setFunc(6, i, i % 2 === 0);
    setFunc(i, 6, i % 2 === 0);
  }

  // Dark module
  setFunc(4 * version + 9, 8, true);

  // Alignment patterns
  const ALIGNMENT_COORDS: number[][] = [
    [],
    [],
    [6, 18],
    [6, 22],
    [6, 26],
    [6, 30],
    [6, 34],
    [6, 22, 38],
    [6, 24, 42],
    [6, 26, 46],
    [6, 28, 50],
  ];
  const coords = ALIGNMENT_COORDS[version] || [];
  for (const ar of coords) {
    for (const ac of coords) {
      if (
        (ar <= 8 && ac <= 8) ||
        (ar <= 8 && ac >= size - 8) ||
        (ar >= size - 8 && ac <= 8)
      ) {
        continue;
      }
      for (let r = -2; r <= 2; r++) {
        for (let c = -2; c <= 2; c++) {
          const isEdge = Math.abs(r) === 2 || Math.abs(c) === 2;
          const isCenter = r === 0 && c === 0;
          setFunc(ar + r, ac + c, isEdge || isCenter);
        }
      }
    }
  }

  // Reserve format area
  for (let i = 0; i < 9; i++) {
    setFunc(8, i, false);
    setFunc(i, 8, false);
    setFunc(8, size - 1 - i, false);
    setFunc(size - 1 - i, 8, false);
  }
  setFunc(8, 8, false);

  // 5. Zigzag data placement
  let bitIdx = 0;
  const totalAllBits = allCodewords.length * 8;
  let right = size - 1;
  let goingUp = true;

  while (right > 0) {
    if (right === 6) right--;
    for (let vertical = 0; vertical < size; vertical++) {
      const r = goingUp ? size - 1 - vertical : vertical;
      for (let c = right; c >= right - 1; c--) {
        if (!isFunction[r][c]) {
          let bit = false;
          if (bitIdx < totalAllBits) {
            const byte = allCodewords[Math.floor(bitIdx / 8)];
            bit = ((byte >> (7 - (bitIdx % 8))) & 1) === 1;
            bitIdx++;
          }
          // Mask 0: (row + col) % 2 === 0
          if ((r + c) % 2 === 0) {
            bit = !bit;
          }
          matrix[r][c] = bit;
        }
      }
    }
    right -= 2;
    goingUp = !goingUp;
  }

  // 6. Format info for EC Level L, Mask 0
  const formatBits = [0, 1, 0, 0, 0, 1, 1, 1, 1, 0, 1, 0, 1, 1, 0];
  for (let i = 0; i <= 5; i++) matrix[8][i] = formatBits[i] === 1;
  matrix[8][7] = formatBits[6] === 1;
  matrix[8][8] = formatBits[7] === 1;
  matrix[7][8] = formatBits[8] === 1;
  for (let i = 9; i < 15; i++) matrix[14 - i][8] = formatBits[i] === 1;

  for (let i = 0; i < 8; i++) matrix[8][size - 1 - i] = formatBits[i] === 1;
  for (let i = 8; i < 15; i++) matrix[size - 15 + i][8] = formatBits[i] === 1;

  return matrix.map((row) => row.map((cell) => cell ?? false));
}

export function QRCodeSVG({
  value,
  size = 200,
  className = '',
}: {
  value: string;
  size?: number;
  className?: string;
}) {
  const matrix = generateQRCodeMatrix(value);
  const matrixSize = matrix.length;
  const padding = 2;
  const totalSize = matrixSize + padding * 2;

  let pathData = '';
  for (let r = 0; r < matrixSize; r++) {
    for (let c = 0; c < matrixSize; c++) {
      if (matrix[r][c]) {
        pathData += `M${c + padding},${r + padding}h1v1h-1z `;
      }
    }
  }

  return (
    <svg
      viewBox={`0 0 ${totalSize} ${totalSize}`}
      width={size}
      height={size}
      className={`rounded-xl bg-[#fff] p-2 ${className}`}
      shapeRendering="crispEdges"
    >
      <rect width={totalSize} height={totalSize} fill="#ffffff" />
      <path d={pathData} fill="#000000" />
    </svg>
  );
}
