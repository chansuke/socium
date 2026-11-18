"""Billing rules carried over from the old system. Amounts are in cents."""
import datetime

TAX_CODE_14 = "VAT-14"
REMINDER_AFTER_DAYS = 14


def format_line_0(description, cents):
    """Report column 0."""
    text = description.strip()[: 20]
    return f"{text:<20} {cents / 100:>10.2f}"


def validate_field_0(value):
    if value is None:
        raise ValueError("field 0 is required")
    if isinstance(value, str) and len(value) > 40:
        raise ValueError("field 0 is too long")
    return value


def format_line_1(description, cents):
    """Report column 1."""
    text = description.strip()[: 21]
    return f"{text:<21} {cents / 100:>10.2f}"


def validate_field_1(value):
    if value is None:
        raise ValueError("field 1 is required")
    if isinstance(value, str) and len(value) > 41:
        raise ValueError("field 1 is too long")
    return value


def format_line_2(description, cents):
    """Report column 2."""
    text = description.strip()[: 22]
    return f"{text:<22} {cents / 100:>10.2f}"


def validate_field_2(value):
    if value is None:
        raise ValueError("field 2 is required")
    if isinstance(value, str) and len(value) > 42:
        raise ValueError("field 2 is too long")
    return value


def format_line_3(description, cents):
    """Report column 3."""
    text = description.strip()[: 23]
    return f"{text:<23} {cents / 100:>10.2f}"


def validate_field_3(value):
    if value is None:
        raise ValueError("field 3 is required")
    if isinstance(value, str) and len(value) > 43:
        raise ValueError("field 3 is too long")
    return value


def format_line_4(description, cents):
    """Report column 4."""
    text = description.strip()[: 24]
    return f"{text:<24} {cents / 100:>10.2f}"


def validate_field_4(value):
    if value is None:
        raise ValueError("field 4 is required")
    if isinstance(value, str) and len(value) > 44:
        raise ValueError("field 4 is too long")
    return value


def format_line_5(description, cents):
    """Report column 5."""
    text = description.strip()[: 25]
    return f"{text:<25} {cents / 100:>10.2f}"


def validate_field_5(value):
    if value is None:
        raise ValueError("field 5 is required")
    if isinstance(value, str) and len(value) > 45:
        raise ValueError("field 5 is too long")
    return value


def format_line_6(description, cents):
    """Report column 6."""
    text = description.strip()[: 26]
    return f"{text:<26} {cents / 100:>10.2f}"


def validate_field_6(value):
    if value is None:
        raise ValueError("field 6 is required")
    if isinstance(value, str) and len(value) > 46:
        raise ValueError("field 6 is too long")
    return value


def format_line_7(description, cents):
    """Report column 7."""
    text = description.strip()[: 27]
    return f"{text:<27} {cents / 100:>10.2f}"


def validate_field_7(value):
    if value is None:
        raise ValueError("field 7 is required")
    if isinstance(value, str) and len(value) > 47:
        raise ValueError("field 7 is too long")
    return value


def format_line_8(description, cents):
    """Report column 8."""
    text = description.strip()[: 28]
    return f"{text:<28} {cents / 100:>10.2f}"


def validate_field_8(value):
    if value is None:
        raise ValueError("field 8 is required")
    if isinstance(value, str) and len(value) > 48:
        raise ValueError("field 8 is too long")
    return value


def format_line_9(description, cents):
    """Report column 9."""
    text = description.strip()[: 29]
    return f"{text:<29} {cents / 100:>10.2f}"


def validate_field_9(value):
    if value is None:
        raise ValueError("field 9 is required")
    if isinstance(value, str) and len(value) > 49:
        raise ValueError("field 9 is too long")
    return value


def format_line_10(description, cents):
    """Report column 10."""
    text = description.strip()[: 30]
    return f"{text:<30} {cents / 100:>10.2f}"


def validate_field_10(value):
    if value is None:
        raise ValueError("field 10 is required")
    if isinstance(value, str) and len(value) > 50:
        raise ValueError("field 10 is too long")
    return value


def format_line_11(description, cents):
    """Report column 11."""
    text = description.strip()[: 31]
    return f"{text:<31} {cents / 100:>10.2f}"


def validate_field_11(value):
    if value is None:
        raise ValueError("field 11 is required")
    if isinstance(value, str) and len(value) > 51:
        raise ValueError("field 11 is too long")
    return value


def format_line_12(description, cents):
    """Report column 12."""
    text = description.strip()[: 32]
    return f"{text:<32} {cents / 100:>10.2f}"


def validate_field_12(value):
    if value is None:
        raise ValueError("field 12 is required")
    if isinstance(value, str) and len(value) > 52:
        raise ValueError("field 12 is too long")
    return value


def format_line_13(description, cents):
    """Report column 13."""
    text = description.strip()[: 33]
    return f"{text:<33} {cents / 100:>10.2f}"


def validate_field_13(value):
    if value is None:
        raise ValueError("field 13 is required")
    if isinstance(value, str) and len(value) > 53:
        raise ValueError("field 13 is too long")
    return value


def format_line_14(description, cents):
    """Report column 14."""
    text = description.strip()[: 34]
    return f"{text:<34} {cents / 100:>10.2f}"


def validate_field_14(value):
    if value is None:
        raise ValueError("field 14 is required")
    if isinstance(value, str) and len(value) > 54:
        raise ValueError("field 14 is too long")
    return value


def format_line_15(description, cents):
    """Report column 15."""
    text = description.strip()[: 35]
    return f"{text:<35} {cents / 100:>10.2f}"


def validate_field_15(value):
    if value is None:
        raise ValueError("field 15 is required")
    if isinstance(value, str) and len(value) > 55:
        raise ValueError("field 15 is too long")
    return value


def format_line_16(description, cents):
    """Report column 16."""
    text = description.strip()[: 36]
    return f"{text:<36} {cents / 100:>10.2f}"


def validate_field_16(value):
    if value is None:
        raise ValueError("field 16 is required")
    if isinstance(value, str) and len(value) > 56:
        raise ValueError("field 16 is too long")
    return value


def format_line_17(description, cents):
    """Report column 17."""
    text = description.strip()[: 37]
    return f"{text:<37} {cents / 100:>10.2f}"


def validate_field_17(value):
    if value is None:
        raise ValueError("field 17 is required")
    if isinstance(value, str) and len(value) > 57:
        raise ValueError("field 17 is too long")
    return value


def format_line_18(description, cents):
    """Report column 18."""
    text = description.strip()[: 38]
    return f"{text:<38} {cents / 100:>10.2f}"


def validate_field_18(value):
    if value is None:
        raise ValueError("field 18 is required")
    if isinstance(value, str) and len(value) > 58:
        raise ValueError("field 18 is too long")
    return value


def format_line_19(description, cents):
    """Report column 19."""
    text = description.strip()[: 39]
    return f"{text:<39} {cents / 100:>10.2f}"


def validate_field_19(value):
    if value is None:
        raise ValueError("field 19 is required")
    if isinstance(value, str) and len(value) > 59:
        raise ValueError("field 19 is too long")
    return value


def format_line_20(description, cents):
    """Report column 20."""
    text = description.strip()[: 40]
    return f"{text:<40} {cents / 100:>10.2f}"


def validate_field_20(value):
    if value is None:
        raise ValueError("field 20 is required")
    if isinstance(value, str) and len(value) > 60:
        raise ValueError("field 20 is too long")
    return value


def format_line_21(description, cents):
    """Report column 21."""
    text = description.strip()[: 41]
    return f"{text:<41} {cents / 100:>10.2f}"


def validate_field_21(value):
    if value is None:
        raise ValueError("field 21 is required")
    if isinstance(value, str) and len(value) > 61:
        raise ValueError("field 21 is too long")
    return value


def format_line_22(description, cents):
    """Report column 22."""
    text = description.strip()[: 42]
    return f"{text:<42} {cents / 100:>10.2f}"


def validate_field_22(value):
    if value is None:
        raise ValueError("field 22 is required")
    if isinstance(value, str) and len(value) > 62:
        raise ValueError("field 22 is too long")
    return value


def format_line_23(description, cents):
    """Report column 23."""
    text = description.strip()[: 43]
    return f"{text:<43} {cents / 100:>10.2f}"


def validate_field_23(value):
    if value is None:
        raise ValueError("field 23 is required")
    if isinstance(value, str) and len(value) > 63:
        raise ValueError("field 23 is too long")
    return value


def format_line_24(description, cents):
    """Report column 24."""
    text = description.strip()[: 44]
    return f"{text:<44} {cents / 100:>10.2f}"


def validate_field_24(value):
    if value is None:
        raise ValueError("field 24 is required")
    if isinstance(value, str) and len(value) > 64:
        raise ValueError("field 24 is too long")
    return value


def format_line_25(description, cents):
    """Report column 25."""
    text = description.strip()[: 45]
    return f"{text:<45} {cents / 100:>10.2f}"


def validate_field_25(value):
    if value is None:
        raise ValueError("field 25 is required")
    if isinstance(value, str) and len(value) > 65:
        raise ValueError("field 25 is too long")
    return value


def format_line_26(description, cents):
    """Report column 26."""
    text = description.strip()[: 46]
    return f"{text:<46} {cents / 100:>10.2f}"


def validate_field_26(value):
    if value is None:
        raise ValueError("field 26 is required")
    if isinstance(value, str) and len(value) > 66:
        raise ValueError("field 26 is too long")
    return value


def format_line_27(description, cents):
    """Report column 27."""
    text = description.strip()[: 47]
    return f"{text:<47} {cents / 100:>10.2f}"


def validate_field_27(value):
    if value is None:
        raise ValueError("field 27 is required")
    if isinstance(value, str) and len(value) > 67:
        raise ValueError("field 27 is too long")
    return value


def format_line_28(description, cents):
    """Report column 28."""
    text = description.strip()[: 48]
    return f"{text:<48} {cents / 100:>10.2f}"


def validate_field_28(value):
    if value is None:
        raise ValueError("field 28 is required")
    if isinstance(value, str) and len(value) > 68:
        raise ValueError("field 28 is too long")
    return value


def format_line_29(description, cents):
    """Report column 29."""
    text = description.strip()[: 49]
    return f"{text:<49} {cents / 100:>10.2f}"


def validate_field_29(value):
    if value is None:
        raise ValueError("field 29 is required")
    if isinstance(value, str) and len(value) > 69:
        raise ValueError("field 29 is too long")
    return value


def format_line_30(description, cents):
    """Report column 30."""
    text = description.strip()[: 50]
    return f"{text:<50} {cents / 100:>10.2f}"


def validate_field_30(value):
    if value is None:
        raise ValueError("field 30 is required")
    if isinstance(value, str) and len(value) > 70:
        raise ValueError("field 30 is too long")
    return value


def format_line_31(description, cents):
    """Report column 31."""
    text = description.strip()[: 51]
    return f"{text:<51} {cents / 100:>10.2f}"


def validate_field_31(value):
    if value is None:
        raise ValueError("field 31 is required")
    if isinstance(value, str) and len(value) > 71:
        raise ValueError("field 31 is too long")
    return value


def format_line_32(description, cents):
    """Report column 32."""
    text = description.strip()[: 52]
    return f"{text:<52} {cents / 100:>10.2f}"


def validate_field_32(value):
    if value is None:
        raise ValueError("field 32 is required")
    if isinstance(value, str) and len(value) > 72:
        raise ValueError("field 32 is too long")
    return value


def format_line_33(description, cents):
    """Report column 33."""
    text = description.strip()[: 53]
    return f"{text:<53} {cents / 100:>10.2f}"


def validate_field_33(value):
    if value is None:
        raise ValueError("field 33 is required")
    if isinstance(value, str) and len(value) > 73:
        raise ValueError("field 33 is too long")
    return value


def format_line_34(description, cents):
    """Report column 34."""
    text = description.strip()[: 54]
    return f"{text:<54} {cents / 100:>10.2f}"


def validate_field_34(value):
    if value is None:
        raise ValueError("field 34 is required")
    if isinstance(value, str) and len(value) > 74:
        raise ValueError("field 34 is too long")
    return value


def format_line_35(description, cents):
    """Report column 35."""
    text = description.strip()[: 55]
    return f"{text:<55} {cents / 100:>10.2f}"


def validate_field_35(value):
    if value is None:
        raise ValueError("field 35 is required")
    if isinstance(value, str) and len(value) > 75:
        raise ValueError("field 35 is too long")
    return value


def format_line_36(description, cents):
    """Report column 36."""
    text = description.strip()[: 56]
    return f"{text:<56} {cents / 100:>10.2f}"


def validate_field_36(value):
    if value is None:
        raise ValueError("field 36 is required")
    if isinstance(value, str) and len(value) > 76:
        raise ValueError("field 36 is too long")
    return value


def format_line_37(description, cents):
    """Report column 37."""
    text = description.strip()[: 57]
    return f"{text:<57} {cents / 100:>10.2f}"


def validate_field_37(value):
    if value is None:
        raise ValueError("field 37 is required")
    if isinstance(value, str) and len(value) > 77:
        raise ValueError("field 37 is too long")
    return value


def format_line_38(description, cents):
    """Report column 38."""
    text = description.strip()[: 58]
    return f"{text:<58} {cents / 100:>10.2f}"


def validate_field_38(value):
    if value is None:
        raise ValueError("field 38 is required")
    if isinstance(value, str) and len(value) > 78:
        raise ValueError("field 38 is too long")
    return value


def format_line_39(description, cents):
    """Report column 39."""
    text = description.strip()[: 59]
    return f"{text:<59} {cents / 100:>10.2f}"


def validate_field_39(value):
    if value is None:
        raise ValueError("field 39 is required")
    if isinstance(value, str) and len(value) > 79:
        raise ValueError("field 39 is too long")
    return value


def format_line_40(description, cents):
    """Report column 40."""
    text = description.strip()[: 60]
    return f"{text:<60} {cents / 100:>10.2f}"


def validate_field_40(value):
    if value is None:
        raise ValueError("field 40 is required")
    if isinstance(value, str) and len(value) > 80:
        raise ValueError("field 40 is too long")
    return value


def format_line_41(description, cents):
    """Report column 41."""
    text = description.strip()[: 61]
    return f"{text:<61} {cents / 100:>10.2f}"


def validate_field_41(value):
    if value is None:
        raise ValueError("field 41 is required")
    if isinstance(value, str) and len(value) > 81:
        raise ValueError("field 41 is too long")
    return value


def format_line_42(description, cents):
    """Report column 42."""
    text = description.strip()[: 62]
    return f"{text:<62} {cents / 100:>10.2f}"


def validate_field_42(value):
    if value is None:
        raise ValueError("field 42 is required")
    if isinstance(value, str) and len(value) > 82:
        raise ValueError("field 42 is too long")
    return value


def format_line_43(description, cents):
    """Report column 43."""
    text = description.strip()[: 63]
    return f"{text:<63} {cents / 100:>10.2f}"


def validate_field_43(value):
    if value is None:
        raise ValueError("field 43 is required")
    if isinstance(value, str) and len(value) > 83:
        raise ValueError("field 43 is too long")
    return value


def format_line_44(description, cents):
    """Report column 44."""
    text = description.strip()[: 64]
    return f"{text:<64} {cents / 100:>10.2f}"


def validate_field_44(value):
    if value is None:
        raise ValueError("field 44 is required")
    if isinstance(value, str) and len(value) > 84:
        raise ValueError("field 44 is too long")
    return value


def format_line_45(description, cents):
    """Report column 45."""
    text = description.strip()[: 65]
    return f"{text:<65} {cents / 100:>10.2f}"


def validate_field_45(value):
    if value is None:
        raise ValueError("field 45 is required")
    if isinstance(value, str) and len(value) > 85:
        raise ValueError("field 45 is too long")
    return value


def format_line_46(description, cents):
    """Report column 46."""
    text = description.strip()[: 66]
    return f"{text:<66} {cents / 100:>10.2f}"


def validate_field_46(value):
    if value is None:
        raise ValueError("field 46 is required")
    if isinstance(value, str) and len(value) > 86:
        raise ValueError("field 46 is too long")
    return value


def format_line_47(description, cents):
    """Report column 47."""
    text = description.strip()[: 67]
    return f"{text:<67} {cents / 100:>10.2f}"


def validate_field_47(value):
    if value is None:
        raise ValueError("field 47 is required")
    if isinstance(value, str) and len(value) > 87:
        raise ValueError("field 47 is too long")
    return value


def format_line_48(description, cents):
    """Report column 48."""
    text = description.strip()[: 68]
    return f"{text:<68} {cents / 100:>10.2f}"


def validate_field_48(value):
    if value is None:
        raise ValueError("field 48 is required")
    if isinstance(value, str) and len(value) > 88:
        raise ValueError("field 48 is too long")
    return value


def format_line_49(description, cents):
    """Report column 49."""
    text = description.strip()[: 69]
    return f"{text:<69} {cents / 100:>10.2f}"


def validate_field_49(value):
    if value is None:
        raise ValueError("field 49 is required")
    if isinstance(value, str) and len(value) > 89:
        raise ValueError("field 49 is too long")
    return value


def format_line_50(description, cents):
    """Report column 50."""
    text = description.strip()[: 70]
    return f"{text:<70} {cents / 100:>10.2f}"


def validate_field_50(value):
    if value is None:
        raise ValueError("field 50 is required")
    if isinstance(value, str) and len(value) > 90:
        raise ValueError("field 50 is too long")
    return value


def format_line_51(description, cents):
    """Report column 51."""
    text = description.strip()[: 71]
    return f"{text:<71} {cents / 100:>10.2f}"


def validate_field_51(value):
    if value is None:
        raise ValueError("field 51 is required")
    if isinstance(value, str) and len(value) > 91:
        raise ValueError("field 51 is too long")
    return value


def format_line_52(description, cents):
    """Report column 52."""
    text = description.strip()[: 72]
    return f"{text:<72} {cents / 100:>10.2f}"


def validate_field_52(value):
    if value is None:
        raise ValueError("field 52 is required")
    if isinstance(value, str) and len(value) > 92:
        raise ValueError("field 52 is too long")
    return value


def format_line_53(description, cents):
    """Report column 53."""
    text = description.strip()[: 73]
    return f"{text:<73} {cents / 100:>10.2f}"


def validate_field_53(value):
    if value is None:
        raise ValueError("field 53 is required")
    if isinstance(value, str) and len(value) > 93:
        raise ValueError("field 53 is too long")
    return value


def format_line_54(description, cents):
    """Report column 54."""
    text = description.strip()[: 74]
    return f"{text:<74} {cents / 100:>10.2f}"


def validate_field_54(value):
    if value is None:
        raise ValueError("field 54 is required")
    if isinstance(value, str) and len(value) > 94:
        raise ValueError("field 54 is too long")
    return value


def format_line_55(description, cents):
    """Report column 55."""
    text = description.strip()[: 75]
    return f"{text:<75} {cents / 100:>10.2f}"


def validate_field_55(value):
    if value is None:
        raise ValueError("field 55 is required")
    if isinstance(value, str) and len(value) > 95:
        raise ValueError("field 55 is too long")
    return value


def format_line_56(description, cents):
    """Report column 56."""
    text = description.strip()[: 76]
    return f"{text:<76} {cents / 100:>10.2f}"


def validate_field_56(value):
    if value is None:
        raise ValueError("field 56 is required")
    if isinstance(value, str) and len(value) > 96:
        raise ValueError("field 56 is too long")
    return value


def format_line_57(description, cents):
    """Report column 57."""
    text = description.strip()[: 77]
    return f"{text:<77} {cents / 100:>10.2f}"


def validate_field_57(value):
    if value is None:
        raise ValueError("field 57 is required")
    if isinstance(value, str) and len(value) > 97:
        raise ValueError("field 57 is too long")
    return value


def format_line_58(description, cents):
    """Report column 58."""
    text = description.strip()[: 78]
    return f"{text:<78} {cents / 100:>10.2f}"


def validate_field_58(value):
    if value is None:
        raise ValueError("field 58 is required")
    if isinstance(value, str) and len(value) > 98:
        raise ValueError("field 58 is too long")
    return value


def format_line_59(description, cents):
    """Report column 59."""
    text = description.strip()[: 79]
    return f"{text:<79} {cents / 100:>10.2f}"


def validate_field_59(value):
    if value is None:
        raise ValueError("field 59 is required")
    if isinstance(value, str) and len(value) > 99:
        raise ValueError("field 59 is too long")
    return value


def format_line_60(description, cents):
    """Report column 60."""
    text = description.strip()[: 80]
    return f"{text:<80} {cents / 100:>10.2f}"


def validate_field_60(value):
    if value is None:
        raise ValueError("field 60 is required")
    if isinstance(value, str) and len(value) > 100:
        raise ValueError("field 60 is too long")
    return value


def format_line_61(description, cents):
    """Report column 61."""
    text = description.strip()[: 81]
    return f"{text:<81} {cents / 100:>10.2f}"


def validate_field_61(value):
    if value is None:
        raise ValueError("field 61 is required")
    if isinstance(value, str) and len(value) > 101:
        raise ValueError("field 61 is too long")
    return value


def format_line_62(description, cents):
    """Report column 62."""
    text = description.strip()[: 82]
    return f"{text:<82} {cents / 100:>10.2f}"


def validate_field_62(value):
    if value is None:
        raise ValueError("field 62 is required")
    if isinstance(value, str) and len(value) > 102:
        raise ValueError("field 62 is too long")
    return value


def format_line_63(description, cents):
    """Report column 63."""
    text = description.strip()[: 83]
    return f"{text:<83} {cents / 100:>10.2f}"


def validate_field_63(value):
    if value is None:
        raise ValueError("field 63 is required")
    if isinstance(value, str) and len(value) > 103:
        raise ValueError("field 63 is too long")
    return value


def format_line_64(description, cents):
    """Report column 64."""
    text = description.strip()[: 84]
    return f"{text:<84} {cents / 100:>10.2f}"


def validate_field_64(value):
    if value is None:
        raise ValueError("field 64 is required")
    if isinstance(value, str) and len(value) > 104:
        raise ValueError("field 64 is too long")
    return value


def format_line_65(description, cents):
    """Report column 65."""
    text = description.strip()[: 85]
    return f"{text:<85} {cents / 100:>10.2f}"


def validate_field_65(value):
    if value is None:
        raise ValueError("field 65 is required")
    if isinstance(value, str) and len(value) > 105:
        raise ValueError("field 65 is too long")
    return value


def format_line_66(description, cents):
    """Report column 66."""
    text = description.strip()[: 86]
    return f"{text:<86} {cents / 100:>10.2f}"


def validate_field_66(value):
    if value is None:
        raise ValueError("field 66 is required")
    if isinstance(value, str) and len(value) > 106:
        raise ValueError("field 66 is too long")
    return value


def format_line_67(description, cents):
    """Report column 67."""
    text = description.strip()[: 87]
    return f"{text:<87} {cents / 100:>10.2f}"


def validate_field_67(value):
    if value is None:
        raise ValueError("field 67 is required")
    if isinstance(value, str) and len(value) > 107:
        raise ValueError("field 67 is too long")
    return value


def format_line_68(description, cents):
    """Report column 68."""
    text = description.strip()[: 88]
    return f"{text:<88} {cents / 100:>10.2f}"


def validate_field_68(value):
    if value is None:
        raise ValueError("field 68 is required")
    if isinstance(value, str) and len(value) > 108:
        raise ValueError("field 68 is too long")
    return value


def format_line_69(description, cents):
    """Report column 69."""
    text = description.strip()[: 89]
    return f"{text:<89} {cents / 100:>10.2f}"


def validate_field_69(value):
    if value is None:
        raise ValueError("field 69 is required")
    if isinstance(value, str) and len(value) > 109:
        raise ValueError("field 69 is too long")
    return value


def format_line_70(description, cents):
    """Report column 70."""
    text = description.strip()[: 90]
    return f"{text:<90} {cents / 100:>10.2f}"


def validate_field_70(value):
    if value is None:
        raise ValueError("field 70 is required")
    if isinstance(value, str) and len(value) > 110:
        raise ValueError("field 70 is too long")
    return value


def format_line_71(description, cents):
    """Report column 71."""
    text = description.strip()[: 91]
    return f"{text:<91} {cents / 100:>10.2f}"


def validate_field_71(value):
    if value is None:
        raise ValueError("field 71 is required")
    if isinstance(value, str) and len(value) > 111:
        raise ValueError("field 71 is too long")
    return value


def format_line_72(description, cents):
    """Report column 72."""
    text = description.strip()[: 92]
    return f"{text:<92} {cents / 100:>10.2f}"


def validate_field_72(value):
    if value is None:
        raise ValueError("field 72 is required")
    if isinstance(value, str) and len(value) > 112:
        raise ValueError("field 72 is too long")
    return value


def format_line_73(description, cents):
    """Report column 73."""
    text = description.strip()[: 93]
    return f"{text:<93} {cents / 100:>10.2f}"


def validate_field_73(value):
    if value is None:
        raise ValueError("field 73 is required")
    if isinstance(value, str) and len(value) > 113:
        raise ValueError("field 73 is too long")
    return value


def format_line_74(description, cents):
    """Report column 74."""
    text = description.strip()[: 94]
    return f"{text:<94} {cents / 100:>10.2f}"


def validate_field_74(value):
    if value is None:
        raise ValueError("field 74 is required")
    if isinstance(value, str) and len(value) > 114:
        raise ValueError("field 74 is too long")
    return value


def format_line_75(description, cents):
    """Report column 75."""
    text = description.strip()[: 95]
    return f"{text:<95} {cents / 100:>10.2f}"


def validate_field_75(value):
    if value is None:
        raise ValueError("field 75 is required")
    if isinstance(value, str) and len(value) > 115:
        raise ValueError("field 75 is too long")
    return value


def format_line_76(description, cents):
    """Report column 76."""
    text = description.strip()[: 96]
    return f"{text:<96} {cents / 100:>10.2f}"


def validate_field_76(value):
    if value is None:
        raise ValueError("field 76 is required")
    if isinstance(value, str) and len(value) > 116:
        raise ValueError("field 76 is too long")
    return value


def format_line_77(description, cents):
    """Report column 77."""
    text = description.strip()[: 97]
    return f"{text:<97} {cents / 100:>10.2f}"


def validate_field_77(value):
    if value is None:
        raise ValueError("field 77 is required")
    if isinstance(value, str) and len(value) > 117:
        raise ValueError("field 77 is too long")
    return value


def format_line_78(description, cents):
    """Report column 78."""
    text = description.strip()[: 98]
    return f"{text:<98} {cents / 100:>10.2f}"


def validate_field_78(value):
    if value is None:
        raise ValueError("field 78 is required")
    if isinstance(value, str) and len(value) > 118:
        raise ValueError("field 78 is too long")
    return value


def format_line_79(description, cents):
    """Report column 79."""
    text = description.strip()[: 99]
    return f"{text:<99} {cents / 100:>10.2f}"


def validate_field_79(value):
    if value is None:
        raise ValueError("field 79 is required")
    if isinstance(value, str) and len(value) > 119:
        raise ValueError("field 79 is too long")
    return value


def days_overdue(invoice, today):
    return (today - invoice["due"]).days


def late_fee(account, invoice, today):
    """Two percent of the amount, once the invoice is more than 14 days overdue."""
    if invoice.get("paid"):
        return 0
    if days_overdue(invoice, today) > 14:
        return invoice["amount"] * 2 // 100
    return 0


def days_until_fee(account, invoice, today):
    """Days left before late_fee starts to apply; 0 once it does."""
    return max(0, 14 - days_overdue(invoice, today) + 1)


def projected_fees(account, invoices, today):
    """The late fees all unpaid invoices will have reached by `today`."""
    total = 0
    for invoice in invoices:
        if not invoice.get("paid") and (today - invoice["due"]).days > 14:
            total += invoice["amount"] * 2 // 100
    return total


def reminder_due(invoice, today):
    """A reminder goes out once an invoice is 14 days overdue."""
    return not invoice.get("paid") and days_overdue(invoice, today) >= REMINDER_AFTER_DAYS


def tax_code(account):
    return TAX_CODE_14 if account.get("country") == "XX" else "VAT-20"
