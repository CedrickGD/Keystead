import { useState } from "react";
import { Trash2 } from "lucide-react";
import { api } from "../../lib/api";
import { useT } from "../../i18n";
import { useApp } from "../../state/app";
import { useIcons } from "../../state/icons";
import { useToast } from "../../components/Toasts";
import { Button, Switch } from "../../components/Controls";
import { SettingRow } from "./SettingsPage";

/**
 * Settings → Allgemein: load website icons automatically (directly from the
 * websites), and delete the icons stored in the open vault.
 */
export function WebsiteIconsSetting() {
  const { t, tp, errorText } = useT();
  const toast = useToast();
  const { settings, updateSettings } = useApp();
  const { icons, refresh } = useIcons();
  const [clearing, setClearing] = useState(false);
  const count = Object.keys(icons).length;

  const clear = async () => {
    setClearing(true);
    try {
      await api.clearIcons();
      toast.success(t(settings.websiteIcons ? "settings.iconsClearedReload" : "settings.iconsCleared"));
    } catch (err) {
      toast.error(errorText(err));
    } finally {
      refresh();
      setClearing(false);
    }
  };

  return (
    <>
      <SettingRow title={t("settings.websiteIcons")} description={t("settings.websiteIconsDesc")}>
        <Switch
          checked={settings.websiteIcons}
          onChange={(websiteIcons) => void updateSettings({ websiteIcons })}
          label={t("settings.websiteIcons")}
        />
      </SettingRow>
      <SettingRow
        title={t("settings.storedIcons")}
        description={count > 0 ? tp("settings.storedIconsCount", count) : t("settings.storedIconsNone")}
      >
        <Button
          variant="ghost"
          size="sm"
          icon={<Trash2 />}
          loading={clearing}
          disabled={count === 0}
          onClick={() => void clear()}
        >
          {t("settings.clearIcons")}
        </Button>
      </SettingRow>
    </>
  );
}
