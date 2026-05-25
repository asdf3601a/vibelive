<template>
  <div class="flex flex-col items-center justify-center py-24 text-center">
    <div class="mx-auto mb-6 flex h-20 w-20 items-center justify-center rounded-full bg-bg-elevated">
      <svg class="h-10 w-10 text-text-muted" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
        <path stroke-linecap="round" stroke-linejoin="round" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
      </svg>
    </div>
    <h1 class="text-3xl font-bold text-text-primary">404</h1>
    <p class="mt-2 text-text-secondary">Page not found</p>
    <router-link
      to="/"
      class="mt-6 inline-flex items-center gap-1 rounded-lg bg-accent-primary px-4 py-2 text-sm font-medium text-white hover:bg-accent-primary/90 transition"
    >
      Go home
    </router-link>
  </div>
</template>
