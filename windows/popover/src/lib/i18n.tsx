import { createContext, useContext, type ReactNode } from "react";
import { m } from "@/paraglide/messages.js";
import { getLocale, setLocale } from "@/paraglide/runtime.js";

export type Language = "en" | "pt-BR";

// Labels that reach the page as data rather than as a message: metric names from the Rust
// report, the provider links and the star-limit error from `model.js`.
const metricMessages: Record<string, (locale: Language) => string> = {
  "Activity": (locale: Language) => m["activity"]({}, { locale }),
  "API Keys": (locale: Language) => m["api_keys"]({}, { locale }),
  "Balance": (locale: Language) => m["balance"]({}, { locale }),
  "Credits": (locale: Language) => m["credits"]({}, { locale }),
  "Dashboard": (locale: Language) => m["dashboard"]({}, { locale }),
  "Monthly": (locale: Language) => m["monthly"]({}, { locale }),
  "Rate Limit Resets": (locale: Language) => m["rate_limit_resets"]({}, { locale }),
  "Session": (locale: Language) => m["session"]({}, { locale }),
  "Status": (locale: Language) => m["status"]({}, { locale }),
  "Up to 2 stars per provider": (locale: Language) => m["up_to_2_stars_per_provider"]({}, { locale }),
  "Usage": (locale: Language) => m["usage"]({}, { locale }),
  "Weekly": (locale: Language) => m["weekly"]({}, { locale }),
};

const LanguageContext = createContext<Language>("en");

export function LanguageProvider({ children, language }: { children: ReactNode; language: Language }) {
  if (getLocale() !== language) setLocale(language, { reload: false });
  return <LanguageContext.Provider value={language}>{children}</LanguageContext.Provider>;
}

export function translateMetricLabel(language: Language, label: string): string {
  if (language !== "pt-BR") return label;
  const parts = /^(.+?) \((.+)\)$/.exec(label);
  if (parts) return `${translateMetricLabel(language, parts[1])} (${translateMetricLabel(language, parts[2])})`;
  return metricMessages[label]?.(language) ?? label;
}

export function translateUsage(language: Language, value: string): string {
  if (language !== "pt-BR") return value;
  const options = { locale: language };
  return value
    .replace(/\b(\d+)% left\b/g, (_, percent: string) => m.percent_left({ percent }, options))
    .replace(/\b(\d+)% used\b/g, (_, percent: string) => m.percent_used({ percent }, options))
    .replace(/\bLimit reached\b/g, m.limit_reached({}, options));
}

export function useI18n() {
  const language = useContext(LanguageContext);
  return { language, metricLabel: (label: string) => translateMetricLabel(language, label) };
}
