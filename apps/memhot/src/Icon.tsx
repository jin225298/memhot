import type { CSSProperties } from "react";
const paths = {
  archive: "M3 3h18v5H3Zm2 5v13h14V8M9 12h6",
  bookmark: "M6 3h12v19l-6-4-6 4Z",
  search: "M10.5 18a7.5 7.5 0 1 1 0-15 7.5 7.5 0 0 1 0 15Zm5.3-2.2L22 22",
  edit: "m16 3 5 5M3 21l5-1L21 7a2 2 0 0 0-4-4L4 16Zm1-5 4 4",
  person: "M16 7a4 4 0 1 1-8 0 4 4 0 0 1 8 0ZM4 21v-2a8 8 0 0 1 16 0v2",
  sun: "M12 3v2m0 14v2M3 12h2m14 0h2M5.6 5.6 7 7m10 10 1.4 1.4M5.6 18.4 7 17M17 7l1.4-1.4M16 12a4 4 0 1 1-8 0 4 4 0 0 1 8 0Z",
  thought: "M8.5 17h7M9 20h6M8.5 17c0-2-3-3-3-7a6.5 6.5 0 0 1 13 0c0 4-3 5-3 7",
  mic: "M9 5a3 3 0 0 1 6 0v7a3 3 0 0 1-6 0ZM5 10v2a7 7 0 0 0 14 0v-2M12 19v3m-4 0h8",
  settings: "M4 7h16M4 17h16M9 4v6m6 4v6",
  plus: "M12 5v14M5 12h14",
  arrow: "M5 12h14m-6-6 6 6-6 6",
  arrowUp: "M12 19V5m-6 6 6-6 6 6",
  back: "M19 12H5m6-6-6 6 6 6",
  external:
    "M14 4h6v6m0-6L10 14M9 4H5a1 1 0 0 0-1 1v14a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-4",
  chat: "M21 11a8 8 0 0 1-8 8H8l-5 3V11a9 9 0 0 1 18 0Z",
  close: "m6 6 12 12M6 18 18 6",
  check: "m5 12 4 4L19 6",
  file: "M14 3H5v18h14V8Zm0 0v5h5M8 12h8m-8 4h6",
  upload: "M12 16V3m-5 5 5-5 5 5M4 16v5h16v-5",
  refresh: "M20 7v5h-5M4 17v-5h5m10.5-3a8 8 0 0 0-14-3M4.5 15a8 8 0 0 0 14 3",
  stop: "M6 6h12v12H6Z",
  send: "m3 3 19 9-19 9 4-9Zm4 9h15",
  clock: "M12 8v5l3 2m6-3a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z",
  trash: "M4 6h16M9 6V3h6v3M6 6l1 15h10l1-15M10 10v7m4-7v7",
  chevron: "m9 5 7 7-7 7",
  book: "M12 5c-4-2-7-2-10-1v15c4-1 7-1 10 1m0-15c4-2 7-2 10-1v15c-4-1-7-1-10 1Zm0 0v15",
} as const;
export type IconName = keyof typeof paths;
export function Icon({
  name,
  size = 20,
  style,
}: {
  name: IconName;
  size?: number;
  style?: CSSProperties;
}) {
  return (
    <svg
      aria-hidden="true"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      style={style}
    >
      <path d={paths[name]} />
    </svg>
  );
}
export function Mark() {
  return (
    <svg
      className="brand-mark"
      aria-hidden="true"
      width="30"
      height="30"
      viewBox="0 0 30 30"
      fill="none"
    >
      <path
        d="M5 23V10a4 4 0 0 1 8 0v10M13 12a4 4 0 0 1 8 0v11M21 16a4 4 0 0 1 4 4v3"
        stroke="currentColor"
        strokeWidth="2.8"
        strokeLinecap="round"
      />
      <circle cx="24" cy="6" r="2" fill="currentColor" />
    </svg>
  );
}
