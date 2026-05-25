import { createRouter, createWebHistory } from 'vue-router'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      path: '/',
      name: 'Home',
      component: () => import('@/views/Home.vue'),
    },
    {
      path: '/live/:key',
      name: 'LiveWatch',
      component: () => import('@/views/LiveWatch.vue'),
    },
    {
      path: '/recordings',
      name: 'Recordings',
      component: () => import('@/views/Recordings.vue'),
    },
    {
      path: '/:pathMatch(.*)*',
      name: 'NotFound',
      component: () => import('@/views/NotFound.vue'),
    },
  ],
})

router.beforeEach((to) => {
  if (to.name === 'Home') {
    document.title = 'LiveStream Platform'
  } else if (to.name === 'LiveWatch') {
    document.title = 'Live Watch — LiveStream Platform'
  } else if (to.name === 'Recordings') {
    document.title = 'Recordings — LiveStream Platform'
  } else if (to.name === 'NotFound') {
    document.title = 'Not Found — LiveStream Platform'
  }
})

export default router
