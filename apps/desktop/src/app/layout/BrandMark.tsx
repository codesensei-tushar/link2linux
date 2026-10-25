import { useId } from "react";

/** The app icon (src-tauri/icons/app-icon.svg) drawn inline. Decorative. */
export function BrandMark({ className }: { className?: string }) {
  const gradientId = useId();
  return (
    <svg className={className} viewBox="64 64 896 896" aria-hidden="true" focusable="false">
      <defs>
        <linearGradient
          id={gradientId}
          gradientUnits="userSpaceOnUse"
          x1="96"
          y1="96"
          x2="928"
          y2="928"
        >
          <stop offset="0" stopColor="#5B5BD6" />
          <stop offset="1" stopColor="#0F9F8F" />
        </linearGradient>
      </defs>
      <rect x="64" y="64" width="896" height="896" rx="208" fill={`url(#${gradientId})`} />
      <g fill="none" stroke="#fff" strokeLinecap="round" strokeLinejoin="round">
        <rect x="384" y="268" width="448" height="344" rx="44" strokeWidth="48" />
        <path d="M608 612v92M520 724h176" strokeWidth="48" />
        <rect
          x="192"
          y="388"
          width="232"
          height="396"
          rx="48"
          fill={`url(#${gradientId})`}
          strokeWidth="48"
        />
        <path d="M532 436c44-48 116-48 160 0M574 488c20-22 56-22 76 0" strokeWidth="40" />
        <circle cx="612" cy="540" r="26" fill="#fff" stroke="none" />
      </g>
    </svg>
  );
}
