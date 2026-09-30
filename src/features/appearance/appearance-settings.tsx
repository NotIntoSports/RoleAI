import { Monitor, Moon, Sun } from "lucide-react";
import { parseLanguagePreference, setLanguagePreference, useLanguagePreference, useT } from "../../i18n";
import { setThemePreference, useThemePreference } from "./theme";

const options = [
  { value: "system", labelKey: "settings.appearance.theme.system.label", descriptionKey: "settings.appearance.theme.system.description", Icon: Monitor },
  { value: "light", labelKey: "settings.appearance.theme.light.label", descriptionKey: "settings.appearance.theme.light.description", Icon: Sun },
  { value: "dark", labelKey: "settings.appearance.theme.dark.label", descriptionKey: "settings.appearance.theme.dark.description", Icon: Moon },
] as const;

const languageOptions = [
  { value: "system", labelKey: "settings.appearance.language.system" },
  { value: "zh-CN", labelKey: "settings.appearance.language.simplifiedChinese" },
  { value: "en", labelKey: "settings.appearance.language.english" },
] as const;

export function AppearanceSettings() {
  const t = useT();
  const preference = useThemePreference();
  const language = useLanguagePreference();
  return (
    <section className="appearance-settings" aria-labelledby="appearance-heading">
      <div className="section-heading"><h2 id="appearance-heading">{t("settings.appearance.heading")}</h2><p className="muted">{t("settings.appearance.description")}</p></div>
      <fieldset className="theme-options">
        <legend className="sr-only">{t("settings.appearance.themeLegend")}</legend>
        {options.map(({ value, labelKey, descriptionKey, Icon }) => (
          <label className="theme-option" data-selected={preference === value} key={value}>
            <input type="radio" name="theme" aria-label={t(labelKey)} value={value} checked={preference === value} onChange={() => setThemePreference(value)} />
            <span className="theme-preview" data-preview={value} aria-hidden="true"><span className="theme-preview-nav"><i /><i /><i /></span><span className="theme-preview-main"><i /><i /><i /><span /></span></span>
            <span className="theme-option-label"><Icon size={16} aria-hidden="true" />{t(labelKey)}</span>
            <span className="theme-option-description">{t(descriptionKey)}</span>
          </label>
        ))}
      </fieldset>
      <fieldset className="appearance-language">
        <legend className="sr-only">{t("settings.appearance.language.legend")}</legend>
        <label>
          {t("settings.appearance.language.legend")}
          <select value={language} onChange={(event) => setLanguagePreference(parseLanguagePreference(event.target.value))}>
            {languageOptions.map(({ value, labelKey }) => (
              <option key={value} value={value}>{t(labelKey)}</option>
            ))}
          </select>
        </label>
        <p className="muted">{t("settings.appearance.language.note")}</p>
      </fieldset>
      <p className="appearance-note muted">{t("settings.appearance.note")}</p>
    </section>
  );
}
