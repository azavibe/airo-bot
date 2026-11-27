// The settings sections and their addresses, apart from the Settings screen
// itself so the app can route to it without loading it.

export type SettingsSection = 'account' | 'appearance' | 'voice' | 'connections' | 'admin';

/** The address of each settings section (the first is plain /settings). */
export const SETTINGS_PATHS: Record<SettingsSection, string> = {
  account: '/settings',
  appearance: '/settings/appearance',
  voice: '/settings/voice',
  connections: '/settings/connections',
  admin: '/settings/admin',
};

export const settingsSectionFromPath = (path: string): SettingsSection => {
  if (path.startsWith('/connections') || path.startsWith('/settings/connections')) return 'connections';
  if (path === '/admin' || path.startsWith('/admin/') || path.startsWith('/settings/admin')) return 'admin';
  if (path.startsWith('/settings/appearance')) return 'appearance';
  if (path.startsWith('/settings/voice')) return 'voice';
  return 'account';
};
