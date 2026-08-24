import {
  AlertTriangle,
  Check,
  ChevronDown,
  LoaderCircle,
  Search,
  X,
  type LucideIcon,
} from "lucide-react";
import {
  type ButtonHTMLAttributes,
  type HTMLAttributes,
  type InputHTMLAttributes,
  type PropsWithChildren,
  type ReactNode,
  type SelectHTMLAttributes,
  useEffect,
  useId,
  useRef,
} from "react";

export function cn(...values: Array<string | false | null | undefined>): string {
  return values.filter(Boolean).join(" ");
}

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: "primary" | "secondary" | "ghost" | "danger" | "link";
  size?: "small" | "medium";
  icon?: LucideIcon;
  loading?: boolean;
}

export function Button({
  variant = "secondary",
  size = "medium",
  icon: Icon,
  loading,
  className,
  children,
  disabled,
  ...props
}: ButtonProps) {
  return (
    <button
      type="button"
      className={cn("button", `button--${variant}`, `button--${size}`, className)}
      disabled={disabled || loading}
      {...props}
    >
      {loading ? <LoaderCircle className="spin" size={15} aria-hidden /> : Icon ? <Icon size={15} aria-hidden /> : null}
      {children}
    </button>
  );
}

export function IconButton({ className, children, ...props }: ButtonHTMLAttributes<HTMLButtonElement>) {
  return <button type="button" className={cn("icon-button", className)} {...props}>{children}</button>;
}

interface SwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  label: string;
}

export function Switch({ checked, onChange, disabled, label }: SwitchProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      className={cn("switch", checked && "switch--checked")}
      disabled={disabled}
      onClick={() => onChange(!checked)}
    >
      <span className="switch__thumb" />
    </button>
  );
}

export function Card({ className, children, ...props }: PropsWithChildren<HTMLAttributes<HTMLDivElement>>) {
  return <div className={cn("card", className)} {...props}>{children}</div>;
}

interface StatusPillProps {
  tone?: "positive" | "warning" | "negative" | "neutral" | "accent";
  children: ReactNode;
  dot?: boolean;
}

export function StatusPill({ tone = "neutral", dot = true, children }: StatusPillProps) {
  return (
    <span className={cn("status-pill", `status-pill--${tone}`)}>
      {dot && <span className="status-pill__dot" aria-hidden />}
      {children}
    </span>
  );
}

interface SectionHeadingProps {
  title: string;
  description?: string;
  trailing?: ReactNode;
}

export function SectionHeading({ title, description, trailing }: SectionHeadingProps) {
  return (
    <div className="section-heading">
      <div>
        <h2>{title}</h2>
        {description && <p>{description}</p>}
      </div>
      {trailing && <div className="section-heading__trailing">{trailing}</div>}
    </div>
  );
}

interface EmptyStateProps {
  icon: LucideIcon;
  title: string;
  description: string;
  action?: ReactNode;
}

export function EmptyState({ icon: Icon, title, description, action }: EmptyStateProps) {
  return (
    <div className="empty-state">
      <div className="empty-state__icon"><Icon size={24} /></div>
      <h3>{title}</h3>
      <p>{description}</p>
      {action}
    </div>
  );
}

interface InlineErrorProps {
  message: string;
  onRetry?: () => void;
  onDismiss?: () => void;
}

export function InlineError({ message, onRetry, onDismiss }: InlineErrorProps) {
  return (
    <div className="inline-error" role="alert">
      <AlertTriangle size={17} aria-hidden />
      <span>{message}</span>
      <div className="inline-error__actions">
        {onRetry && <Button variant="ghost" size="small" onClick={onRetry}>重试</Button>}
        {onDismiss && <IconButton aria-label="关闭错误" onClick={onDismiss}><X size={15} /></IconButton>}
      </div>
    </div>
  );
}

interface SearchFieldProps extends Omit<InputHTMLAttributes<HTMLInputElement>, "type"> {
  compact?: boolean;
}

export function SearchField({ className, compact, ...props }: SearchFieldProps) {
  return (
    <label className={cn("search-field", compact && "search-field--compact", className)}>
      <Search size={15} aria-hidden />
      <input type="search" {...props} />
    </label>
  );
}

interface SegmentedControlProps<T extends string> {
  value: T;
  options: Array<{ value: T; label: string }>;
  onChange: (value: T) => void;
  label: string;
}

export function SegmentedControl<T extends string>({ value, options, onChange, label }: SegmentedControlProps<T>) {
  return (
    <div className="segmented" role="tablist" aria-label={label}>
      {options.map((option) => (
        <button
          type="button"
          role="tab"
          aria-selected={value === option.value}
          className={cn("segmented__item", value === option.value && "segmented__item--active")}
          onClick={() => onChange(option.value)}
          key={option.value}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

export function Select({ children, className, ...props }: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <span className={cn("select-wrap", className)}>
      <select {...props}>{children}</select>
      <ChevronDown size={14} aria-hidden />
    </span>
  );
}

interface FieldProps {
  label: string;
  hint?: string;
  children: ReactNode;
  htmlFor?: string;
}

export function Field({ label, hint, children, htmlFor }: FieldProps) {
  return (
    <label className="field" htmlFor={htmlFor}>
      <span className="field__label">{label}</span>
      {children}
      {hint && <p className="field__hint">{hint}</p>}
    </label>
  );
}

interface SettingsRowProps {
  title: string;
  description?: string;
  control: ReactNode;
}

export function SettingsRow({ title, description, control }: SettingsRowProps) {
  return (
    <div className="settings-row">
      <div>
        <h3>{title}</h3>
        {description && <p>{description}</p>}
      </div>
      <div className="settings-row__control">{control}</div>
    </div>
  );
}

interface ModalProps {
  open: boolean;
  title: string;
  description?: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  size?: "small" | "medium" | "large";
}

export function Modal({ open, title, description, onClose, children, footer, size = "medium" }: ModalProps) {
  const titleId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    const previous = document.activeElement as HTMLElement | null;
    window.setTimeout(() => dialogRef.current?.focus(), 0);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      previous?.focus();
    };
  }, [onClose, open]);
  if (!open) return null;
  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <div
        ref={dialogRef}
        className={cn("modal", `modal--${size}`)}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
      >
        <header className="modal__header">
          <div>
            <h2 id={titleId}>{title}</h2>
            {description && <p>{description}</p>}
          </div>
          <IconButton aria-label="关闭" onClick={onClose}><X size={17} /></IconButton>
        </header>
        <div className="modal__body">{children}</div>
        {footer && <footer className="modal__footer">{footer}</footer>}
      </div>
    </div>
  );
}

interface ToastProps {
  message: string;
  onClose: () => void;
}

export function Toast({ message, onClose }: ToastProps) {
  return (
    <div className="toast" role="status">
      <Check size={16} aria-hidden />
      <span>{message}</span>
      <IconButton aria-label="关闭提示" onClick={onClose}><X size={14} /></IconButton>
    </div>
  );
}
