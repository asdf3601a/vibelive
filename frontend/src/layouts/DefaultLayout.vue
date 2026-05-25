<template>
  <div class="min-h-screen flex flex-col bg-bg-base text-text-primary font-sans">
    <nav class="border-b border-border-default bg-bg-surface/80 backdrop-blur sticky top-0 z-50">
      <div class="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
        <div class="flex h-14 items-center justify-between">
          <router-link
            to="/"
            class="flex items-center gap-2.5 text-white font-bold text-lg tracking-tight hover:opacity-80 transition shrink-0"
          >
            <span class="inline-flex h-7 w-7 items-center justify-center rounded-md bg-accent-live text-[10px] font-bold">
              ●
            </span>
            LiveStream
          </router-link>

          <div class="flex items-center gap-3 text-sm text-text-secondary flex-nowrap">
            <NavStatus />
          </div>
        </div>
      </div>
    </nav>

    <main class="mx-auto max-w-7xl w-full px-4 sm:px-6 lg:px-8 py-6 flex-1">
      <router-view v-slot="{ Component }">
        <Transition name="fade" mode="out-in">
          <component :is="Component" />
        </Transition>
      </router-view>
    </main>

    <footer class="border-t border-border-default mt-auto py-4 text-center text-xs text-text-muted">
      RTMP Ingest → fMP4/HLS
    </footer>
  </div>
</template>

<script setup lang="ts">
import NavStatus from '@/components/NavStatus.vue'
</script>

<style>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.2s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
