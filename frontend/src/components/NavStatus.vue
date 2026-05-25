<template>
  <div class="flex items-center gap-2 text-xs">
    <span class="hidden sm:inline-flex items-center gap-1.5 rounded-full bg-bg-elevated px-2.5 py-1 font-medium whitespace-nowrap text-text-secondary border border-border-default">
      <span class="h-1.5 w-1.5 rounded-full bg-accent-success animate-pulse"></span>
      RTMP 1935
    </span>
    <span class="hidden sm:inline-flex items-center gap-1.5 rounded-full bg-bg-elevated px-2.5 py-1 font-medium whitespace-nowrap text-text-secondary border border-border-default">
      <span class="h-1.5 w-1.5 rounded-full bg-accent-primary"></span>
      HLS 8080
    </span>
    <router-link
      to="/recordings"
      class="hidden sm:inline-flex items-center gap-1.5 rounded-full bg-bg-elevated px-2.5 py-1 font-medium whitespace-nowrap text-text-secondary border border-border-default hover:bg-bg-overlay hover:text-text-primary transition"
    >
      <svg class="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
        <path stroke-linecap="round" stroke-linejoin="round" d="M15 10l4.553-4.553A1 1 0 0121 6.12V17.88a1 1 0 01-1.447.894L15 14M5 18h8a2 2 0 002-2V8a2 2 0 00-2-2H5a2 2 0 00-2 2v8a2 2 0 002 2z" />
      </svg>
      Recordings
    </router-link>
  </div>
</template>
