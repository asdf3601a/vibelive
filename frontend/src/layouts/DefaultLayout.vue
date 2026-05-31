<template>
  <div class="min-h-screen flex flex-col bg-bg-base text-text-primary font-sans">
    <nav class="border-b border-border-default bg-bg-surface/80 backdrop-blur sticky top-0 z-50">
      <div class="mx-auto px-4 sm:px-6 lg:px-8 content-container">
        <div class="flex h-14 items-center justify-between">
          <router-link
            to="/"
            class="flex items-center gap-2.5 text-white font-bold text-lg tracking-tight hover:opacity-80 transition shrink-0"
          >
            <img src="/icon.svg" alt="LiveStream" class="h-7 w-7 rounded-md" />
            LiveStream
          </router-link>

          <div class="flex items-center gap-3 text-sm text-text-secondary flex-nowrap">
            <NavStatus />
          </div>
        </div>
      </div>
    </nav>

    <main class="mx-auto w-full px-4 sm:px-6 lg:px-8 py-6 flex-1 content-container">
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

.content-container {
  max-width: 80rem;
}

@media (min-width: 1920px) and (min-aspect-ratio: 16/9) {
  .content-container {
    max-width: min(60%, calc(100vh * 16 / 9 * 0.6));
  }
}
</style>
