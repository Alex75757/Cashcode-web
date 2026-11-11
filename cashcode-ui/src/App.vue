<script setup lang="ts">
import { useWebSocket } from '@vueuse/core'
import { computed, ref, watch } from 'vue'

// Типы сообщений от бэкенда
interface WsEvent {
  type: 'Welcome' | 'Prompt' | 'BillEscrowed' | 'BillStacked' | 'BillReturned' | 'Error' | 'DeviceReady'
  data?: any
}

const wsUrl = 'ws://localhost:3000/ws'
const { status, data, send, open } = useWebSocket(wsUrl, {
  autoReconnect: true,
  onConnected: () => console.log('WS connected'),
  onDisconnected: () => console.log('WS disconnected'),
})

// Парсим последнее сообщение
const lastEvent = computed<WsEvent | null>(() => {
  if (!data.value) return null
  try {
    return JSON.parse(data.value) as WsEvent
  } catch {
    return null
  }
})

// Состояние UI
const promptText = ref('Подключение...')
const billResult = ref<string | null>(null)
const errorText = ref<string | null>(null)
const isReady = ref(false)

// Реагируем на события от бэкенда
watch(lastEvent, (event) => {
  if (!event) return
  switch (event.type) {
    case 'Welcome':
      promptText.value = 'Ожидание инициализации устройства...'
      break
    case 'DeviceReady':
      isReady.value = true
      break
    case 'Prompt':
      promptText.value = event.data.message
      billResult.value = null
      errorText.value = null
      break
    case 'BillEscrowed':
      promptText.value = 'Купюра распознана, принимаем...'
      break
    case 'BillStacked':
      billResult.value = `✅ Принята купюра (тип ${event.data.bill_type})`
      break
    case 'BillReturned':
      billResult.value = `↩️ Купюра возвращена (тип ${event.data.bill_type})`
      break
    case 'Error':
      errorText.value = event.data.message
      promptText.value = 'Ошибка. Попробуйте позже.'
      break
  }
})

const statusText = computed(() => {
  if (status.value === 'OPEN') return '🟢 Онлайн'
  if (status.value === 'CONNECTING') return '🟡 Подключение...'
  return '🔴 Офлайн'
})
</script>

<template>
  <div class="container">
    <h1>Оплата наличными</h1>
    <div class="status">{{ statusText }}</div>

    <div class="card">
      <div class="icon">💵</div>
      <h2>{{ promptText }}</h2>

      <div v-if="billResult" class="result success">
        {{ billResult }}
      </div>
      <div v-if="errorText" class="result error">
        {{ errorText }}
      </div>

      <button v-if="!isReady" @click="open">Переподключиться</button>
    </div>

    <div class="hint">
      Вставьте купюру в купюроприёмник. Она будет автоматически принята.
    </div>
  </div>
</template>

<style scoped>
.container {
  max-width: 500px;
  margin: 50px auto;
  font-family: system-ui, sans-serif;
  text-align: center;
  color: #333;
}
.status {
  margin-bottom: 20px;
  font-size: 14px;
  color: #666;
}
.card {
  background: #f5f5f5;
  border-radius: 16px;
  padding: 40px 20px;
  box-shadow: 0 4px 12px rgba(0,0,0,0.1);
}
.icon {
  font-size: 64px;
  margin-bottom: 16px;
}
h2 {
  font-size: 20px;
  margin-bottom: 24px;
}
.result {
  padding: 12px 20px;
  border-radius: 8px;
  margin-top: 16px;
  font-weight: 500;
}
.success {
  background: #e6ffe6;
  color: #006600;
}
.error {
  background: #ffe6e6;
  color: #cc0000;
}
button {
  margin-top: 20px;
  padding: 10px 24px;
  background: #007bff;
  color: white;
  border: none;
  border-radius: 8px;
  cursor: pointer;
}
.hint {
  margin-top: 30px;
  font-size: 13px;
  color: #999;
}
</style>