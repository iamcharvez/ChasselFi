#include <WiFi.h>
#include <HTTPClient.h>
#include <ArduinoJson.h>

// Install ArduinoJson, then replace every value below before flashing.
static const char *WIFI_SSID = "CHASSELFI-VLAN799";
static const char *WIFI_PASSWORD = "REPLACE_ME";
static const char *NODE_ID = "vendo-01";
static const char *NODE_KEY = "REPLACE_WITH_CHASSELFI_COIN_NODE_KEY";
static const char *API = "http://10.0.0.1:2081/api/coin-node";
static const int COIN_PULSE_PIN = 27;
static const int ACCEPTOR_ENABLE_PIN = 26;
static const bool ENABLE_ACTIVE_HIGH = true;

volatile uint32_t pendingPulses = 0;
volatile uint32_t lastPulseMicros = 0;
String activeClaim;
uint32_t eventCounter = 0;
uint32_t bootId = 0;
unsigned long lastStatusAt = 0;
unsigned long lastHeartbeatAt = 0;

void IRAM_ATTR onCoinPulse() {
  const uint32_t now = micros();
  if (now - lastPulseMicros > 50000) { // 50 ms debounce
    pendingPulses++;
    lastPulseMicros = now;
  }
}

void setAcceptor(bool enabled) {
  digitalWrite(ACCEPTOR_ENABLE_PIN, enabled == ENABLE_ACTIVE_HIGH ? HIGH : LOW);
}

bool request(const String &url, const char *method, const String &body, JsonDocument &reply) {
  if (WiFi.status() != WL_CONNECTED) return false;
  HTTPClient http;
  http.setTimeout(1500);
  if (!http.begin(url)) return false;
  http.addHeader("Content-Type", "application/json");
  http.addHeader("X-ChasselFi-Coin-Key", NODE_KEY);
  int code = strcmp(method, "POST") == 0 ? http.POST(body) : http.GET();
  String response = http.getString();
  http.end();
  if (code < 200 || code >= 300) return false;
  return deserializeJson(reply, response) == DeserializationError::Ok;
}

void heartbeat() {
  JsonDocument reply;
  String body = String("{\"nodeId\":\"") + NODE_ID + "\",\"firmware\":\"esp32-1.0.0\"}";
  request(String(API) + "/heartbeat", "POST", body, reply);
}

void refreshClaim() {
  JsonDocument reply;
  bool ok = request(String(API) + "/status?nodeId=" + NODE_ID, "GET", "", reply);
  if (!ok || !reply["accepting"].as<bool>()) {
    activeClaim = "";
    setAcceptor(false);
    return;
  }
  activeClaim = reply["claim"]["claimId"].as<String>();
  setAcceptor(activeClaim.length() > 0);
}

void submitPulse() {
  noInterrupts();
  uint32_t count = pendingPulses;
  pendingPulses = 0;
  interrupts();
  if (count == 0) return;
  if (activeClaim.length() == 0) return; // fail closed; pulse is not credited

  eventCounter++;
  String eventId = String("boot") + bootId + "-pulse" + eventCounter;
  String body = String("{\"nodeId\":\"") + NODE_ID +
    "\",\"claimId\":\"" + activeClaim +
    "\",\"eventId\":\"" + eventId +
    "\",\"count\":" + count + "}";

  // Keep the same eventId while retrying so the server can deduplicate it.
  for (int attempt = 0; attempt < 5; attempt++) {
    JsonDocument reply;
    if (request(String(API) + "/pulse", "POST", body, reply) && reply["accepted"].as<bool>()) {
      if (reply["completed"].as<bool>()) {
        activeClaim = "";
        setAcceptor(false);
      }
      return;
    }
    delay(250);
  }
  setAcceptor(false); // operator intervention is safer than duplicate credit
}

void setup() {
  Serial.begin(115200);
  bootId = esp_random();
  pinMode(ACCEPTOR_ENABLE_PIN, OUTPUT);
  setAcceptor(false);
  pinMode(COIN_PULSE_PIN, INPUT_PULLUP);
  attachInterrupt(digitalPinToInterrupt(COIN_PULSE_PIN), onCoinPulse, FALLING);
  WiFi.mode(WIFI_STA);
  WiFi.begin(WIFI_SSID, WIFI_PASSWORD);
}

void loop() {
  if (WiFi.status() != WL_CONNECTED) {
    setAcceptor(false);
    delay(250);
    return;
  }
  const unsigned long now = millis();
  if (now - lastHeartbeatAt >= 10000) { lastHeartbeatAt = now; heartbeat(); }
  if (now - lastStatusAt >= 300) { lastStatusAt = now; refreshClaim(); }
  submitPulse();
  delay(10);
}
