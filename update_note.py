import json
import requests

ANKI_CONNECT_URL = "http://127.0.0.1:8765"
DECK_NAME = "Glossika-ENJA [2001-3000]"

def anki_request(action, **params):
    """
    Helper function to send requests to AnkiConnect.
    """
    payload = {"action": action, "params": params, "version": 6}
    try:
        response = requests.post(ANKI_CONNECT_URL, data=json.dumps(payload))
        response.raise_for_status()
        response_json = response.json()
        if response_json.get("error"):
            raise Exception(f"AnkiConnect API error: {response_json['error']}")
        return response_json.get("result")
    except requests.exceptions.RequestException as e:
        raise Exception(f"Could not connect to AnkiConnect. Is Anki running? Error: {e}")

def main():
    target_glossika_id = "2001"

    print("=" * 60)
    print(f"Finding note with Glossika ID: {target_glossika_id}")
    print("=" * 60)

    try:
        # Search for the note with Id field = 2001
        query = f'deck:"{DECK_NAME}" Id:{target_glossika_id}'
        note_ids = anki_request('findNotes', query=query)

        if not note_ids:
            print(f"\n✗ No note found with Glossika ID '{target_glossika_id}'")
            return

        print(f"\n✓ Found {len(note_ids)} note(s) with Glossika ID '{target_glossika_id}'")

        # Get the note info
        note_id = note_ids[0]
        notes_info = anki_request('notesInfo', notes=[note_id])
        note = notes_info[0]

        print(f"\nNote ID: {note['noteId']}")
        print(f"Model: {note['modelName']}")

        # Display current English field
        current_english = note['fields']['English']['value']
        print(f"\nCurrent English field:")
        print(f"  {current_english}")

        # Add "foo" to the English field
        new_english = current_english + " foo"

        print(f"\nNew English field:")
        print(f"  {new_english}")

        # Update the note
        print("\nUpdating note...")
        update_payload = {
            "note": {
                "id": note_id,
                "fields": {
                    "English": new_english
                }
            }
        }

        result = anki_request('updateNote', **update_payload)
        print("✓ Successfully updated note!")

        # Verify the change
        print("\nVerifying change...")
        updated_notes = anki_request('notesInfo', notes=[note_id])
        updated_english = updated_notes[0]['fields']['English']['value']
        print(f"Verified English field:")
        print(f"  {updated_english}")

    except Exception as e:
        print(f"\n✗ Error: {e}")

if __name__ == "__main__":
    main()
