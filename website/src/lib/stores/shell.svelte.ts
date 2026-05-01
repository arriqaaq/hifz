import type { Observation, Memory, Session, Run } from '$lib/types';

export type DrawerItem =
  | {
      kind: 'observation';
      id: string;
      data: Observation;
      onFilterToSession?: (sessionId: string) => void;
    }
  | { kind: 'memory'; id: string; data: Memory }
  | { kind: 'session'; id: string; data: Session }
  | { kind: 'run'; id: string; data: Run };

const PANEL_KEY = 'hifz.panelOpen';

class ShellState {
  panelOpen = $state(true);
  drawerOpen = $state(false);
  drawerItem = $state<DrawerItem | null>(null);
  selectedObsId = $state<string | null>(null);
  commandOpen = $state(false);
  refreshKey = $state(0);

  constructor() {
    if (typeof localStorage !== 'undefined') {
      const v = localStorage.getItem(PANEL_KEY);
      if (v !== null) this.panelOpen = v !== 'false';
    }
  }

  togglePanel() {
    this.panelOpen = !this.panelOpen;
    if (typeof localStorage !== 'undefined') {
      localStorage.setItem(PANEL_KEY, String(this.panelOpen));
    }
  }

  toggleCommand() {
    this.commandOpen = !this.commandOpen;
  }

  openDrawer(item: DrawerItem) {
    this.drawerItem = item;
    this.drawerOpen = true;
  }

  closeDrawer() {
    this.drawerOpen = false;
    this.drawerItem = null;
  }

  bumpRefresh() {
    this.refreshKey += 1;
  }
}

export const shell = new ShellState();
