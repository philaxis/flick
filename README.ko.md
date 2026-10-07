<p align="center"><a href="README.md">English</a> · <a href="README.ko.md">한국어</a> · <a href="README.zh.md">中文</a></p>

<p align="center"><img src="assets/icon-256.png" width="96" alt="Flick"></p>

<p align="center"><strong><a href="https://github.com/philaxis/flick/releases/latest/download/Flick.exe">⬇ Windows용 Flick 받기</a></strong></p>

<p align="center">버튼을 누른 채 마우스를 밀면 옆 데스크톱입니다.</p>

<p align="center"><a href="https://philaxis.github.io/flick/">Flick 웹사이트에서 동작 보기</a></p>

---

**데스크톱이 격자가 됩니다.** 좌우는 한 작업의 화면들, 위아래는 다른 작업입니다.

**누른 채 밀기.** 트리거를 누르고 마우스를 밀면 그 방향으로 한 칸 넘어갑니다. 커서는 제자리에 있습니다.

**눌렀다 떼면 전체 보기.** 지금 칸의 창들과 작은 지도가 뜹니다. 창을 지도의 칸으로 끌면 옮겨집니다.

**버튼은 마음대로.** 처음에는 마우스 "앞으로" 버튼입니다. 트레이 아이콘 우클릭 → *트리거 버튼 바꾸기*에서 원하는 버튼이나 키 여러 개를 누르면 그걸로 바뀝니다.

**작고 무료.** 1MB도 안 되는 실행 파일 하나. 가입도 설치 화면도 없습니다. 오픈 소스(MIT).

## 시작하기

1. 위 버튼으로 받아 실행합니다. 스스로 설치하고 윈도우와 함께 시작합니다.
2. 트리거를 누른 채 마우스를 옆으로 밉니다.
3. 트리거를 눌렀다 떼고, 지도의 **+**로 데스크톱이나 새 행을 추가합니다.

## 알아 둘 것

- 지금은 윈도우 11 23H2 전용입니다. 24H2는 아직 지원하지 않습니다.
- 제거: 설정 → 앱 → 설치된 앱 → Flick.

## 빌드

윈도우에서 `cargo build --release`, WSL에서는 `scripts/build-wsl.sh build --release`.

[MIT](LICENSE). `vendor/winvd`는 [VirtualDesktopAccessor](https://github.com/Ciantic/VirtualDesktopAccessor)(MIT)에서 가져왔습니다.
