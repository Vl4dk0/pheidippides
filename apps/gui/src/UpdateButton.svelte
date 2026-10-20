<script lang="ts">
  import { onMount } from 'svelte';
  import { check, type Update } from '@tauri-apps/plugin-updater';
  import { relaunch } from '@tauri-apps/plugin-process';
  import { CircleArrowUp } from '@lucide/svelte';
  import IconButton from './IconButton.svelte';

  // Updates are offered, never forced: one quiet rail icon appears only when a
  // signed release is available, and installing waits for the human's click.
  let update = $state<Update | null>(null);
  let installing = $state(false);
  let failed = $state(false);

  onMount(() => {
    // Offline, unreleased or development builds have nothing to offer; a failed
    // check must not add noise to the calm UI.
    check()
      .then((found) => (update = found))
      .catch(() => {});
  });

  async function install() {
    if (!update) return;
    installing = true;
    failed = false;
    try {
      await update.downloadAndInstall();
      await relaunch();
    } catch {
      failed = true;
    } finally {
      installing = false;
    }
  }
</script>

{#if update}<IconButton
    icon={CircleArrowUp}
    label={installing
      ? `Installing ${update.version}…`
      : failed
        ? `Update to ${update.version} failed · retry`
        : `Update available · install ${update.version} and restart`}
    active={!failed}
    disabled={installing}
    onclick={() => void install()}
  />{/if}
