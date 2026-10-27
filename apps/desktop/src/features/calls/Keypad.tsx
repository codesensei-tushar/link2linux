const KEYS: [string, string][] = [
  ["1", ""],
  ["2", "ABC"],
  ["3", "DEF"],
  ["4", "GHI"],
  ["5", "JKL"],
  ["6", "MNO"],
  ["7", "PQRS"],
  ["8", "TUV"],
  ["9", "WXYZ"],
  ["*", ""],
  ["0", "+"],
  ["#", ""],
];

/** A phone keypad. `label` names the group, such as "Dial pad". */
export function Keypad({
  label,
  disabled = false,
  onKey,
}: {
  label: string;
  disabled?: boolean;
  onKey: (key: string) => void;
}) {
  return (
    <div className="keypad" role="group" aria-label={label}>
      {KEYS.map(([key, letters]) => (
        <button
          key={key}
          type="button"
          className="keypad__key"
          aria-label={key}
          disabled={disabled}
          onClick={() => {
            onKey(key);
          }}
        >
          <span className="keypad__digit">{key}</span>
          <span className="keypad__letters" aria-hidden="true">
            {letters}
          </span>
        </button>
      ))}
    </div>
  );
}
