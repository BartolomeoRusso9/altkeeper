# AltKeeper 한국어 웹 UI 번역 검수안 (적용 전)

> 상태: **초안 / 미적용** — 현재 HTML/JS의 언어는 영어 그대로 유지. 이 문서를 검수하고 승인받은 뒤 번역 구현 PR로 반영합니다.

## 번역 원칙

- UI의 **AltKeeper**, **Apple ID**, **Discord**, **IPA**, **UUID**, **KST** 등 식별자와 제품명은 번역하지 않음.
- `Renew`는 '갱신', `Install`은 '설치', `Provisioning profile`은 '프로비저닝 프로파일'로 통일.
- 이 앱 목록은 휴대전화의 프로파일을 기반으로 하므로, 설치 앱 조회에 실패했을 때도 표시할 수 있음. 따라서 '설치된 앱' 대신 **앱 목록** 사용.
- 실패 로그·Apple 원문 오류는 정확한 문제 진단을 위해 번역보다는 원문 유지 + 짧은 한국어 설명 병기를 권장.
- 권장 구현은 **한국어 / English 전환**. 한국어를 원하는 사용자에게 한국어 기본값을 제공하면서 영어 원문도 선택 가능해야 함. 자동 감지 여부/기본값은 검수 후 확정.

## 주요 UI 문구

| 영어 원문 | 제안 한국어 |
|---|---|
| Keeps your iPhone apps alive by renewing their profiles. | 아이폰 앱의 프로파일을 갱신해 사용 기간을 유지합니다. |
| iPhone | 아이폰 |
| Checking… | 확인 중… |
| iPhone reachable | 아이폰 연결됨 |
| iPhone unreachable | 아이폰 연결 안 됨 |
| Not checked yet | 아직 확인하지 않음 |
| Check again | 다시 확인 |
| Your apps | 앱 목록 |
| Renew app | 이 앱 갱신 |
| Details | 상세 정보 |
| Less | 접기 |
| Renew expiring apps | 만료 임박 앱 갱신 |
| Renew all now | 전체 갱신 |
| Preview renewals | 갱신 대상 미리보기 |
| Automatic renewal | 자동 갱신 |
| OFF | 꺼짐 |
| ON | 켜짐 |
| Automatic renewal disabled | 자동 갱신이 꺼져 있습니다 |
| Daily time (KST) | 매일 실행 시간 (KST) |
| Renew when days remaining | 만료까지 남은 기간이 |
| 1 day / 2 days / … / 6 days | 1일 / 2일 / … / 6일 |
| Enable auto-renew | 자동 갱신 켜기 |
| Save settings | 설정 저장 |
| Turn off | 끄기 |
| Discord failure notifications | Discord 실패 알림 |
| Not configured | 설정 안 됨 |
| Configured | 설정됨 |
| Save webhook | 웹훅 저장 |
| Clear webhook | 웹훅 삭제 |
| Send test notification | 테스트 알림 보내기 |
| Test notification sent. Check your Discord channel. | 테스트 알림을 보냈습니다. Discord 채널을 확인하세요. |
| Configure a Discord webhook first. | 먼저 Discord 웹훅을 설정하세요. |
| Provisioning profiles on iPhone | 아이폰의 프로비저닝 프로파일 |
| Delete profile | 프로파일 삭제 |
| Show | 펼치기 |
| Hide | 접기 |
| Saved profile backups | 저장된 프로파일 백업 |
| Restore | 복원 |
| Install an app from this PC | 이 PC에서 IPA 설치 |
| IPA file | IPA 파일 |
| Sign and install | 서명 후 설치 |
| Uploading… | 업로드 중… |
| Apple ID (email) | Apple ID (이메일) |
| Password | 비밀번호 |
| Show password | 비밀번호 표시 |
| Change account | 계정 변경 |
| Sign in | 로그인 |
| Code | 인증 코드 |
| Submit code | 코드 확인 |
| Resend code | 코드 다시 보내기 |
| Recent renewals | 최근 갱신 기록 |
| For use on your home network only. | 가정 내 네트워크에서만 사용하세요. |

## 설명 문구 (의미 보존 필요)

| 위치 | 제안 한국어 |
|---|---|
| 자동 갱신 안내 | 서버가 실행 중일 때 하루 한 번 만료가 임박한 앱을 갱신합니다. 설치된 앱을 조회하지 못하면 저장된 Apple Team의 모든 프로파일을 갱신 대상으로 사용합니다. 기본값은 꺼짐입니다. |
| 자동 갱신 재시도 | 예약된 갱신에 실패하면 30분 간격으로 최대 2회 재시도합니다. 모든 시도가 실패했거나 재시도할 수 없는 인증 오류일 때만 Discord 알림을 보냅니다. |
| Discord 알림 안내 | 자동 갱신이 최종 실패했을 때만 알림을 보냅니다. |
| 프로파일 삭제 안내 | 갱신 후에도 이전 프로파일이 남을 수 있습니다. 사용 중인 프로파일을 삭제하면 앱이 실행되지 않을 수 있습니다. 삭제 전에 서버에 검증된 백업을 저장합니다. |
| 프로파일 복원 안내 | UUID로 백업 프로파일을 복원합니다. 복원해도 원래 만료일은 연장되지 않으며, 백업 파일은 유지됩니다. |
| IPA 설치 안내 | IPA를 선택하면 저장된 Apple ID로 서명한 뒤 Wi-Fi를 통해 아이폰에 설치합니다. Apple ID 비밀번호를 서버에 저장해야 합니다. 설치하는 동안 아이폰 화면을 잠금 해제한 상태로 유지하는 것을 권장합니다. |
| Apple ID 저장 안내 | 자동 갱신을 위해 서버에 비밀번호를 저장합니다. 비밀번호는 접근이 제한된 서버 파일에 평문으로 저장됩니다. |
| Apple 로그인 주의 | 한 번에 한 번만 로그인하세요. 반복된 실패는 Apple의 일시적인 로그인 제한을 유발할 수 있습니다. |
| 최근 갱신 기록 없음 | 아직 기록이 없습니다. 자동 갱신을 설정하면 매일 지정된 시간에 실행됩니다. |
| 페어링 누락 | 아이폰 페어링 정보가 없습니다. USB 연결로 페어링한 뒤 README 안내를 따르세요. |

## 동적 상태 메시지 제안

| 영어 문구 | 한국어 제안 |
|---|---|
| expires in 6 days | 6일 후 만료 |
| expires today | 오늘 만료 |
| expired 1 day ago | 1일 전 만료 |
| 2 profiles | 프로파일 2개 |
| Renewing… | 갱신 중… |
| Another operation is in progress. | 다른 작업이 진행 중입니다. |
| The iPhone must be reachable on Wi-Fi. | 아이폰이 Wi-Fi로 연결되어 있어야 합니다. |
| Nothing to renew | 갱신할 항목이 없습니다 |
| Installation completed. | 설치가 완료되었습니다. |
| Error: Installation failed: … | 설치 실패: … (원본 오류 보존) |

## 검수 시 결정할 항목

1. '프로비저닝 프로파일'은 기술적으로 정확하지만 길어 UI에선 '프로파일'로 줄일지 여부.
2. 한국어 기본 표시 vs 브라우저 언어 자동 감지 vs 수동 선택(영어로 다시 바꿀 수 있도록).
3. '앱 목록'의 앱 표시는 프로파일 기반이며, 실제 설치 여부를 항상 확정하지 않는다는 설명을 보일지.
4. 일본어/영어로 출력되는 기기 진단 로그는 **원문 보존**할지, 한국어 요약을 함께 제공할지.

**승인 전에는 한국어 문구를 앱 코드에 반영하지 않습니다.**
